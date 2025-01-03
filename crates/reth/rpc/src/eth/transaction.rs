//! Loads and formats Strata transaction RPC response.

use alloy_primitives::{Bytes, B256};
use reth_node_api::FullNodeComponents;
use reth_provider::{BlockReaderIdExt, TransactionsProvider};
use reth_rpc_eth_api::{
    helpers::{EthSigner, EthTransactions, LoadTransaction, SpawnBlocking},
    FromEthApiError, FullEthApiTypes,
};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthStateCache};
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};

use crate::{SequencerClient, StrataEthApi};

impl<N> EthTransactions for StrataEthApi<N>
where
    Self: LoadTransaction,
    N: FullNodeComponents,
{
    fn provider(&self) -> impl BlockReaderIdExt {
        self.inner.provider()
    }

    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner>>> {
        self.inner.signers()
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    async fn send_raw_transaction(&self, tx: Bytes) -> Result<B256, Self::Error> {
        let recovered = recover_raw_transaction(tx.clone())?;
        let pool_transaction =
            <Self::Pool as TransactionPool>::Transaction::from_pooled(recovered.into());

        // On Strata, transactions are forwarded directly to the sequencer to be included in
        // blocks that it builds.
        if let Some(client) = self.raw_tx_forwarder().as_ref() {
            tracing::debug!( target: "rpc::eth",  "forwarding raw transaction to");
            let _ = client.forward_raw_transaction(&tx).await.inspect_err(|err| {
                    tracing::debug!(target: "rpc::eth", %err, hash=% *pool_transaction.hash(), "failed to forward raw transaction");
                });
        }

        // submit the transaction to the pool with a `Local` origin
        let hash = self
            .pool()
            .add_transaction(TransactionOrigin::Local, pool_transaction)
            .await
            .map_err(Self::Error::from_eth_err)?;

        Ok(hash)
    }
}

impl<N> LoadTransaction for StrataEthApi<N>
where
    Self: SpawnBlocking + FullEthApiTypes,
    N: FullNodeComponents,
{
    type Pool = N::Pool;

    fn provider(&self) -> impl TransactionsProvider {
        self.inner.provider()
    }

    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }

    fn pool(&self) -> &Self::Pool {
        self.inner.pool()
    }
}

impl<N> StrataEthApi<N>
where
    N: FullNodeComponents,
{
    /// Sets a [`SequencerClient`] for `eth_sendRawTransaction` to forward transactions to.
    pub fn set_sequencer_client(
        &self,
        sequencer_client: SequencerClient,
    ) -> Result<(), tokio::sync::SetError<SequencerClient>> {
        self.sequencer_client.set(sequencer_client)
    }

    /// Returns the [`SequencerClient`] if one is set.
    pub fn raw_tx_forwarder(&self) -> Option<SequencerClient> {
        self.sequencer_client.get().cloned()
    }
}
