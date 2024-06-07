use std::io;
use std::process;
use std::sync::Arc;
use std::thread;
use std::time;

use alpen_vertex_consensus_logic::chain_tip;
use anyhow::Context;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::*;

use alpen_vertex_common::logging;
use alpen_vertex_consensus_logic::ctl::CsmController;
use alpen_vertex_consensus_logic::message::{ChainTipMessage, CsmMessage};
use alpen_vertex_consensus_logic::worker;
use alpen_vertex_db::traits::*;
use alpen_vertex_primitives::{block_credential, params::*};
use alpen_vertex_rpc_api::AlpenApiServer;
use alpen_vertex_state::consensus::ConsensusState;
use alpen_vertex_state::operation;

use crate::args::Args;

mod args;
mod rpc_server;

#[derive(Debug, Error)]
pub enum InitError {
    #[error("io: {0}")]
    Io(#[from] io::Error),

    #[error("{0}")]
    Other(String),
}

fn main() {
    let args: Args = argh::from_env();
    if let Err(e) = main_inner(args) {
        eprintln!("FATAL ERROR: {e}");
    }
}

fn main_inner(args: Args) -> anyhow::Result<()> {
    logging::init();

    // Open the database.
    let rbdb = open_rocksdb_database(&args)?;

    // Set up block params.
    let params = Params {
        rollup: RollupParams {
            block_time: 1000,
            cred_rule: block_credential::CredRule::Unchecked,
        },
        run: RunParams {
            l1_follow_distance: 6,
        },
    };
    let params = Arc::new(params);

    // Initialize databases.
    let l1_db = Arc::new(alpen_vertex_db::L1Db::new(rbdb.clone()));
    let l2_db = Arc::new(alpen_vertex_db::stubs::l2::StubL2Db::new()); // FIXME stub
    let sync_ev_db = Arc::new(alpen_vertex_db::SyncEventDb::new(rbdb.clone()));
    let cs_db = Arc::new(alpen_vertex_db::ConsensusStateDb::new(rbdb.clone()));
    let database = Arc::new(alpen_vertex_db::database::CommonDatabase::new(
        l1_db, l2_db, sync_ev_db, cs_db,
    ));

    // Fetch current states of things.
    let cur_sync_idx = database
        .sync_event_provider()
        .get_last_idx()?
        .unwrap_or_default();

    let cw_state = worker::WorkerState::open(params, database.clone())?;

    // Create dataflow channels.
    let (ctm_tx, ctm_rx) = mpsc::channel::<ChainTipMessage>(64);
    let (csm_tx, csm_rx) = mpsc::channel::<CsmMessage>(64);
    let csm_ctl = Arc::new(CsmController::new(database.clone(), csm_tx));
    let (cout_tx, cout_rx) = mpsc::channel::<operation::ConsensusOutput>(64);
    let (cur_state_tx, cur_state_rx) = watch::channel::<Option<ConsensusState>>(None);

    // Init engine controller.
    let eng_ctl = alpen_vertex_evmctl::stub::StubController::new(time::Duration::from_millis(100));
    let eng_ctl = Arc::new(eng_ctl);

    // Start worker threads.
    let cw_handle = thread::spawn(|| worker::consensus_worker_task(cw_state, eng_ctl, csm_rx));

    // Start runtime for async IO tasks.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("vertex")
        .build()
        .expect("init: build rt");

    if let Err(e) = rt.block_on(main_task(args)) {
        error!(err = %e, "main task exited");
        process::exit(0); // special case exit once we've gotten to this point
    }

    info!("exiting");
    Ok(())
}

async fn main_task(args: Args) -> anyhow::Result<()> {
    let (stop_tx, stop_rx) = oneshot::channel();

    // Init RPC methods.
    let alp_rpc = rpc_server::AlpenRpcImpl::new(stop_tx);
    let methods = alp_rpc.into_rpc();

    let rpc_port = args.rpc_port; // TODO make configurable
    let rpc_server = jsonrpsee::server::ServerBuilder::new()
        .build(format!("127.0.0.1:{rpc_port}"))
        .await
        .expect("init: build rpc server");

    let rpc_handle = rpc_server.start(methods);
    info!("started RPC server");

    // Wait for a stop signal.
    let _ = stop_rx.await;

    // Now start shutdown tasks.
    if rpc_handle.stop().is_err() {
        warn!("RPC server already stopped");
    }

    Ok(())
}

fn open_rocksdb_database(args: &Args) -> anyhow::Result<Arc<rockbound::DB>> {
    let mut database_dir = args.datadir.clone();
    database_dir.push("rocksdb");

    let dbname = alpen_vertex_db::ROCKSDB_NAME;
    let cfs = alpen_vertex_db::STORE_COLUMN_FAMILIES;
    let opts = rocksdb::Options::default();

    let rbdb = rockbound::DB::open(
        &database_dir,
        dbname,
        cfs.iter().map(|s| s.to_string()),
        &opts,
    )
    .context("opening database")?;

    Ok(Arc::new(rbdb))
}
