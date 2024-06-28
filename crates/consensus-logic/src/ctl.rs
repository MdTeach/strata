use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tracing::*;

use alpen_vertex_db::{errors::DbError, traits::*};
use alpen_vertex_state::{block::L2Block, sync_event::SyncEvent};

use crate::message::CsmMessage;

/// Controller handle for the consensus state machine.  Used to submit new sync
/// events for persistence and processing.
pub struct CsmController {
    submit_event_shim: SubmitEventShim,
    csm_tx: mpsc::Sender<CsmMessage>,
}

impl CsmController {
    pub fn new<D: Database + Sync + Send + 'static>(
        database: Arc<D>,
        pool: Arc<threadpool::ThreadPool>,
        csm_tx: mpsc::Sender<CsmMessage>,
    ) -> Self {
        let submit_event_shim = make_write_event_shim(database, pool);
        Self {
            submit_event_shim,
            csm_tx,
        }
    }

    /// Writes a sync event to the database and updates the watch channel to
    /// trigger the CSM executor to process the event.
    pub fn submit_event(&self, sync_event: SyncEvent) -> anyhow::Result<()> {
        let ev_idx = self.submit_event_shim.submit_event_blocking(sync_event)?;
        let msg = CsmMessage::EventInput(ev_idx);
        if self.csm_tx.blocking_send(msg).is_err() {
            warn!(%ev_idx, "sync event receiver closed when submitting sync event");
        }

        Ok(())
    }

    /// Writes a sync event to the database and updates the watch channel to
    /// trigger the CSM executor to process the event.
    pub async fn submit_event_async(&self, sync_event: SyncEvent) -> anyhow::Result<()> {
        let ev_idx = self.submit_event_shim.submit_event(sync_event).await?;
        let msg = CsmMessage::EventInput(ev_idx);
        if self.csm_tx.send(msg).await.is_err() {
            warn!(%ev_idx, "sync event receiver closed when submitting sync event");
        }

        Ok(())
    }
}

struct SubmitEventShim {
    handle: Box<dyn Fn(SyncEvent) -> EventSubmitHandle + Sync + Send + 'static>,
}

impl SubmitEventShim {
    /// Synchronously submits an event to the CSM database to be processed by
    /// the thing.
    fn submit_event_blocking(&self, ev: SyncEvent) -> anyhow::Result<u64, DbError> {
        Ok((self.handle)(ev).wait_blocking()?)
    }

    /// Asynchronously submits an event to the CSM database to be processed by
    /// the thing.
    async fn submit_event(&self, ev: SyncEvent) -> anyhow::Result<u64, DbError> {
        Ok((self.handle)(ev).wait().await?)
    }
}

struct EventSubmitHandle {
    resp_rx: oneshot::Receiver<Result<u64, DbError>>,
}

impl EventSubmitHandle {
    pub fn wait_blocking(self) -> Result<u64, DbError> {
        match self.resp_rx.blocking_recv() {
            Ok(v) => v,
            Err(e) => Err(DbError::Other(format!("{e}"))),
        }
    }

    pub async fn wait(self) -> Result<u64, DbError> {
        match self.resp_rx.await {
            Ok(v) => v,
            Err(e) => Err(DbError::Other(format!("{e}"))),
        }
    }
}

fn make_write_event_shim<D: Database + Sync + Send + 'static>(
    database: Arc<D>,
    pool: Arc<threadpool::ThreadPool>,
) -> SubmitEventShim {
    let fun = move |ev| {
        let db = database.clone();
        let (resp_tx, resp_rx) = oneshot::channel();

        pool.execute(move || {
            let se_store = db.sync_event_store();
            let res = se_store.write_sync_event(ev);
            if resp_tx.send(res).is_err() {
                warn!("failed to submit event");
            }
        });

        EventSubmitHandle { resp_rx }
    };

    SubmitEventShim {
        handle: Box::new(fun),
    }
}