import logging as log
import math
import os
import subprocess
import time
from dataclasses import dataclass
from threading import Thread
from typing import Any, Callable, List, Optional, TypeVar

from bitcoinlib.services.bitcoind import BitcoindClient
from strata_utils import convert_to_xonly_pk, musig_aggregate_pks

from constants import *


def generate_jwt_secret() -> str:
    return os.urandom(32).hex()


def generate_blocks(
    bitcoin_rpc: BitcoindClient,
    wait_dur,
    addr: str,
) -> Thread:
    thr = Thread(
        target=generate_task,
        args=(
            bitcoin_rpc,
            wait_dur,
            addr,
        ),
    )
    thr.start()
    return thr


def generate_task(rpc: BitcoindClient, wait_dur, addr):
    while True:
        time.sleep(wait_dur)
        try:
            rpc.proxy.generatetoaddress(1, addr)
        except Exception as ex:
            log.warning(f"{ex} while generating to address {addr}")
            return


def generate_n_blocks(bitcoin_rpc: BitcoindClient, n: int):
    addr = bitcoin_rpc.proxy.getnewaddress()
    print(f"generating {n} blocks to address", addr)
    try:
        blk = bitcoin_rpc.proxy.generatetoaddress(n, addr)
        print("made blocks", blk)
    except Exception as ex:
        log.warning(f"{ex} while generating address")
        return


def wait_until(
    fn: Callable[[], Any],
    error_with: str = "Timed out",
    timeout: int = 5,
    step: float = 0.5,
):
    """
    Wait until a function call returns truth value, given time step, and timeout.
    This function waits until function call returns truth value at the interval of 1 sec
    """
    for _ in range(math.ceil(timeout / step)):
        try:
            if not fn():
                raise Exception
            return
        except Exception as _:
            pass
        time.sleep(step)
    raise AssertionError(error_with)


T = TypeVar("T")


def wait_until_with_value(
    fn: Callable[..., T],
    predicate: Callable[[T], bool],
    error_with: str = "Timed out",
    timeout: int = 5,
    step: float = 0.5,
) -> T:
    """
    Similar to `wait_until` but this returns the value of the function.
    This also takes another predicate which acts on the function value and returns a bool
    """
    for _ in range(math.ceil(timeout / step)):
        try:
            r = fn()
            if not predicate(r):
                raise Exception
            return r
        except Exception as _:
            pass
        time.sleep(step)
    raise AssertionError(error_with)


@dataclass
class ManualGenBlocksConfig:
    btcrpc: BitcoindClient
    finality_depth: int
    gen_addr: str


@dataclass
class RollupParamsSettings:
    block_time_sec: int
    epoch_slots: int
    genesis_trigger: int
    proof_timeout: Optional[int] = None

    # NOTE: type annotation: Ideally we would use `Self` but couldn't use it
    # even after changing python version to 3.12
    @classmethod
    def new_default(cls) -> "RollupParamsSettings":
        return cls(
            block_time_sec=DEFAULT_BLOCK_TIME_SEC,
            epoch_slots=DEFAULT_EPOCH_SLOTS,
            genesis_trigger=DEFAULT_GENESIS_TRIGGER_HT,
            proof_timeout=DEFAULT_PROOF_TIMEOUT,
        )


def check_nth_checkpoint_finalized(
    idx,
    seqrpc,
    manual_gen: ManualGenBlocksConfig | None = None,
    proof_timeout: int | None = None,
):
    """
    This check expects nth checkpoint to be finalized

    Params:
        - idx: The index of checkpoint
        - seqrpc: The sequencer rpc
        - manual_gen: If we need to generate blocks manually
    """
    syncstat = seqrpc.strata_syncStatus()

    # Wait until we find our expected checkpoint.
    batch_info = wait_until_with_value(
        lambda: seqrpc.strata_getCheckpointInfo(idx),
        predicate=lambda v: v is not None,
        error_with=f"Could not find checkpoint info for index {idx}",
        timeout=3,
    )

    assert (
        syncstat["finalized_block_id"] != batch_info["l2_blockid"]
    ), "Checkpoint block should not yet finalize"
    assert batch_info["idx"] == idx
    checkpoint_info_next = seqrpc.strata_getCheckpointInfo(idx + 1)
    assert checkpoint_info_next is None, f"There should be no checkpoint info for {idx + 1} index"

    to_finalize_blkid = batch_info["l2_blockid"]

    # Submit checkpoint if proof_timeout is not set
    if proof_timeout is None:
        submit_checkpoint(idx, seqrpc, manual_gen)
    else:
        # Just wait until timeout period instead of submitting so that sequencer submits empty proof
        delta = 1
        time.sleep(proof_timeout + delta)

    if manual_gen:
        # Produce l1 blocks until proof is finalized
        manual_gen.btcrpc.proxy.generatetoaddress(
            manual_gen.finality_depth + 1, manual_gen.gen_addr
        )

    # Check if finalized
    wait_until(
        lambda: seqrpc.strata_syncStatus()["finalized_block_id"] == to_finalize_blkid,
        error_with="Block not finalized",
        timeout=10,
    )


def submit_checkpoint(idx: int, seqrpc, manual_gen: ManualGenBlocksConfig | None = None):
    """
    Submits checkpoint and if manual_gen, waits till it is present in l1
    """
    last_published_txid = seqrpc.strata_l1status()["last_published_txid"]

    # Post checkpoint proof
    # FIXME/NOTE: Since operating in timeout mode is supported, i.e. sequencer
    # will post empty post if prover doesn't submit proofs in time, we can send
    # empty proof hex.
    # NOTE: The functional tests for verifying proofs need to provide non-empty
    # proofs
    proof_hex = ""

    # This is arbitrary
    seqrpc.strataadmin_submitCheckpointProof(idx, proof_hex)

    # Wait a while for it to be posted to l1. This will happen when there
    # is a new published txid in l1status
    published_txid = wait_until_with_value(
        lambda: seqrpc.strata_l1status()["last_published_txid"],
        predicate=lambda v: v != last_published_txid,
        error_with="Proof was not published to bitcoin",
        timeout=5,
    )

    if manual_gen:
        manual_gen.btcrpc.proxy.generatetoaddress(1, manual_gen.gen_addr)

        # Check it is confirmed
        wait_until(
            lambda: manual_gen.btcrpc.proxy.gettransaction(published_txid)["confirmations"] > 0,
            timeout=5,
            error_with="Published inscription not confirmed",
        )


def check_submit_proof_fails_for_nonexistent_batch(seqrpc, nonexistent_batch: int):
    """
    This check requires that subnitting nonexistent batch proof fails
    """
    proof_hex = ""

    try:
        seqrpc.strataadmin_submitCheckpointProof(nonexistent_batch, proof_hex)
    except Exception as e:
        if hasattr(e, "code"):
            assert e.code == ERROR_CHECKPOINT_DOESNOT_EXIST
        else:
            print("Unexpected error occurred")
            raise e
    else:
        raise AssertionError("Expected rpc error")


def get_logger(name: str, level=log.DEBUG) -> log.Logger:
    logger = log.getLogger(name)

    if not logger.handlers:
        handler = log.StreamHandler()
        logger.setLevel(level)
        formatter = log.Formatter(
            "%(asctime)s - %(name)s - %(levelname)s - %(filename)s:%(lineno)d - %(message)s"
        )
        handler.setFormatter(formatter)

        # Add the handler to the logger
        logger.addHandler(handler)

    return logger


def wait_for_proof_with_time_out(prover_client_rpc, task_id, time_out=3600):
    """
    Waits for a proof task to complete within a specified timeout period.

    This function continuously polls the status of a proof task identified by `task_id` using
    the `prover_client_rpc` client. It checks the status every 2 seconds and waits until the
    proof task status is "Completed" or the specified `time_out` (in seconds) is reached.
    """

    start_time = time.time()
    while True:
        # Fetch the proof status
        proof_status = prover_client_rpc.dev_strata_getTaskStatus(task_id)
        assert proof_status is not None
        print(f"Got the proof status {proof_status}")
        if proof_status == "Completed":
            print(f"Completed the proof generation for {task_id}")
            break

        time.sleep(2)
        elapsed_time = time.time() - start_time  # Calculate elapsed time
        if elapsed_time >= time_out:
            raise TimeoutError(f"Operation timed out after {time_out} seconds.")


def generate_seed_at(path: str):
    """Generates a seed file at specified path."""
    # fmt: off
    cmd = [
        "strata-datatool",
        "-b", "regtest",
        "genseed",
        "-f", path
    ]
    # fmt: on

    res = subprocess.run(cmd, stdout=subprocess.PIPE)
    res.check_returncode()


def generate_seqpubkey_from_seed(path: str) -> str:
    """Generates a sequencer pubkey from the seed at file path."""
    # fmt: off
    cmd = [
        "strata-datatool",
        "-b", "regtest",
        "genseqpubkey",
        "-f", path
    ]
    # fmt: on

    with open(path) as f:
        print("sequencer root privkey", f.read())

    res = subprocess.run(cmd, stdout=subprocess.PIPE)
    res.check_returncode()
    res = str(res.stdout, "utf8").strip()
    assert len(res) > 0, "no output generated"
    print("SEQ PUBKEY", res)
    return res


def generate_opxpub_from_seed(path: str) -> str:
    """Generates operate pubkey from seed at file path."""
    # fmt: off
    cmd = [
        "strata-datatool",
        "-b", "regtest",
        "genopxpub",
        "-f", path
    ]
    # fmt: on

    res = subprocess.run(cmd, stdout=subprocess.PIPE)
    res.check_returncode()
    res = str(res.stdout, "utf8").strip()
    assert len(res) > 0, "no output generated"
    return res


def generate_params(settings: RollupParamsSettings, seqpubkey: str, oppubkeys: list[str]) -> str:
    """Generates a params file from config values."""
    # fmt: off
    cmd = [
        "strata-datatool",
        "-b", "regtest",
        "genparams",
        "--name", "alpenstrata",
        "--block-time", str(settings.block_time_sec),
        "--epoch-slots", str(settings.epoch_slots),
        "--genesis-trigger-height", str(settings.genesis_trigger),
        "--seqkey", seqpubkey,
    ]
    if settings.proof_timeout is not None:
        cmd.extend(["--proof-timeout", str(settings.proof_timeout)])
    # fmt: on

    for k in oppubkeys:
        cmd.extend(["--opkey", k])

    res = subprocess.run(cmd, stdout=subprocess.PIPE)
    res.check_returncode()
    res = str(res.stdout, "utf8").strip()
    assert len(res) > 0, "no output generated"
    return res


def generate_simple_params(
    base_path: str,
    settings: RollupParamsSettings,
    operator_cnt: int,
) -> dict:
    """
    Creates a network with params data and a list of operator seed paths.

    Result options are `params` and `opseedpaths`.
    """
    seqseedpath = os.path.join(base_path, "seqkey.bin")
    opseedpaths = [os.path.join(base_path, "opkey%s.bin") % i for i in range(operator_cnt)]
    for p in [seqseedpath] + opseedpaths:
        generate_seed_at(p)

    seqkey = generate_seqpubkey_from_seed(seqseedpath)
    opxpubs = [generate_opxpub_from_seed(p) for p in opseedpaths]

    params = generate_params(settings, seqkey, opxpubs)
    print("Params", params)
    return {"params": params, "opseedpaths": opseedpaths}


def broadcast_tx(btcrpc: BitcoindClient, outputs: List[dict], options: dict) -> str:
    """
    Broadcast a transaction to the Bitcoin network.
    """
    psbt_result = btcrpc.proxy.walletcreatefundedpsbt([], outputs, 0, options)
    psbt = psbt_result["psbt"]

    signed_psbt = btcrpc.proxy.walletprocesspsbt(psbt)

    finalized_psbt = btcrpc.proxy.finalizepsbt(signed_psbt["psbt"])
    deposit_tx = finalized_psbt["hex"]

    txid = btcrpc.sendrawtransaction(deposit_tx).get("txid", "")

    return txid


def get_bridge_pubkey(seqrpc) -> str:
    """
    Get the bridge pubkey from the sequencer.
    """
    # Wait for seq
    wait_until(
        lambda: seqrpc.strata_protocolVersion() is not None,
        error_with="Sequencer did not start on time",
    )
    op_pks = seqrpc.strata_getActiveOperatorChainPubkeySet()
    print(f"Operator pubkeys: {op_pks}")
    # This returns a dict with index as key and pubkey as value
    # Iterate all of them ant then call musig_aggregate_pks
    # Also since they are full pubkeys, we need to convert them
    # to X-only pubkeys.
    op_pks = [op_pks[str(i)] for i in range(len(op_pks))]
    op_x_only_pks = [convert_to_xonly_pk(pk) for pk in op_pks]
    agg_pubkey = musig_aggregate_pks(op_x_only_pks)
    return agg_pubkey
