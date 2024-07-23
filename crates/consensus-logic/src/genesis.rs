use std::{sync::Arc, thread, time::Duration};

use tracing::*;

use alpen_vertex_db::{errors::DbError, traits::*};
use alpen_vertex_primitives::{
    buf::{Buf32, Buf64},
    l1::L1BlockManifest,
    params::Params,
};
use alpen_vertex_state::{
    block::{ExecSegment, L1Segment},
    chain_state::ChainState,
    client_state::{ClientState, SyncState},
    exec_env::ExecEnvState,
    exec_update::{ExecUpdate, UpdateInput, UpdateOutput},
    header::L2BlockHeader,
    l1::{L1HeaderRecord, L1ViewState},
    prelude::*,
};

use crate::errors::Error;

/// Inserts into the database an initial basic client state that we can begin
/// waiting for genesis with.
pub fn init_client_state(params: &Params, database: &impl Database) -> anyhow::Result<()> {
    debug!("initializing client state in database!");

    let init_state = ClientState::from_genesis_params(
        params.rollup().horizon_l1_height,
        params.rollup().genesis_l1_height,
    );

    // Write the state into the database.
    let cs_store = database.client_state_store();
    cs_store.write_client_state_checkpoint(0, init_state)?;

    Ok(())
}

/// Inserts approprate chainstate into the database to start actively syncing
/// the rollup chain.  Requires that the L1 blocks between the horizon and the
/// L2 genesis are already in the datatabase.
///
/// This does not update the client state to include the new sync state data
/// that it should have now.  That is introduced by writing a new sync event for
/// that.
pub fn init_genesis_chainstate(
    params: &Params,
    database: &impl Database,
) -> anyhow::Result<L2BlockId> {
    debug!("preparing database genesis chainstate!");

    let horizon_blk_height = params.rollup.horizon_l1_height;
    let genesis_blk_height = params.rollup.genesis_l1_height;

    // Query the pre-genesis blocks we need before we do anything else.
    let l1_prov = database.l1_provider();
    let pregenesis_mfs =
        load_pre_genesis_l1_manifests(l1_prov.as_ref(), horizon_blk_height, genesis_blk_height)?;

    // Create a dummy exec state that we can build the rest of the genesis block
    // around and insert into the genesis state.
    // TODO this might need to talk to the EL to do the genesus setup *properly*
    let geui = UpdateInput::new(0, Buf32::zero(), Vec::new());
    let gees = ExecEnvState::from_base_input(geui.clone(), Buf32::zero());
    let genesis_ee_state = Buf32::zero();
    let geu = ExecUpdate::new(geui.clone(), UpdateOutput::new_from_state(genesis_ee_state));

    // Build the genesis block and genesis consensus states.
    let gblock = make_genesis_block(params, geu);
    let genesis_blkid = gblock.header().get_blockid();
    info!(?genesis_blkid, "created genesis block");

    let horizon_blk_rec = L1HeaderRecord::from(pregenesis_mfs.last().unwrap());
    let l1vs = L1ViewState::new_at_horizon(horizon_blk_height, horizon_blk_rec);

    let gchstate = ChainState::from_genesis(genesis_blkid, l1vs, gees);

    // Now insert things into the database.
    let chs_store = database.chainstate_store();
    let l2store = database.l2_store();
    chs_store.write_genesis_state(&gchstate)?;
    l2store.put_block_data(gblock)?;

    // TODO make ^this be atomic so we can't accidentally write both, or make it
    // so we can overwrite the genesis chainstate if there's no other states or
    // something

    info!("finished genesis insertions");
    Ok(genesis_blkid)
}

fn load_pre_genesis_l1_manifests(
    l1_prov: &impl L1DataProvider,
    horizon_height: u64,
    genesis_height: u64,
) -> anyhow::Result<Vec<L1BlockManifest>> {
    let mut manifests = Vec::new();
    for height in horizon_height..=genesis_height {
        let Some(mf) = l1_prov.get_block_manifest(height)? else {
            return Err(Error::MissingL1BlockHeight(height).into());
        };

        manifests.push(mf);
    }

    Ok(manifests)
}

fn make_genesis_block(params: &Params, genesis_update: ExecUpdate) -> L2Block {
    // This has to be empty since everyone should have an unambiguous view of the genesis block.
    let l1_seg = L1Segment::new_empty();

    // TODO this is a total stub, we have to fill it in with something
    let exec_seg = ExecSegment::new(genesis_update);

    let body = L2BlockBody::new(l1_seg, exec_seg);

    // Assemble the genesis header core, pulling in data from whatever
    // sources we need.
    // FIXME this isn't the right timestamp to start the blockchain, this should
    // definitely be pulled from the database or the rollup parameters maybe
    let genesis_ts = params.rollup().horizon_l1_height;
    let zero_blkid = L2BlockId::from(Buf32::zero());
    let genesis_sr = Buf32::zero();
    let header = L2BlockHeader::new(0, genesis_ts, zero_blkid, &body, genesis_sr);
    let signed_genesis_header = SignedL2BlockHeader::new(header, Buf64::zero());
    L2Block::new(signed_genesis_header, body)
}

/// Check if the database needs to have client init done to it.
pub fn check_needs_client_init(database: &impl Database) -> anyhow::Result<bool> {
    let cs_prov = database.client_state_provider();

    // Check if we've written any genesis state checkpoint.  These we perform a
    // bit more carefully and check errors more granularly.
    match cs_prov.get_last_checkpoint_idx() {
        Ok(_) => {}
        Err(DbError::NotBootstrapped) => return Ok(true),

        // TODO should we return an error here or skip?
        Err(e) => return Err(e.into()),
    }

    let l2_prov = database.l2_provider();

    // Check if there's any genesis block written.
    match l2_prov.get_blocks_at_height(0) {
        Ok(blkids) => {
            if blkids.is_empty() {
                return Ok(true);
            }
        }

        Err(DbError::NotBootstrapped) => return Ok(true),

        // Again, how should we handle this?
        Err(e) => return Err(e.into()),
    }

    Ok(false)
}

pub fn check_needs_genesis(database: &impl Database) -> anyhow::Result<bool> {
    let l2_prov = database.l2_provider();

    // Check if there's any genesis block written.
    match l2_prov.get_blocks_at_height(0) {
        Ok(blkids) => Ok(blkids.is_empty()),

        Err(DbError::NotBootstrapped) => Ok(true),

        // Again, how should we handle this?
        Err(e) => Err(e.into()),
    }
}
