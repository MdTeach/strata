use alpen_vertex_primitives::prelude::*;

use crate::block::L2BlockId;

/// Sync event that updates our consensus state.
#[derive(Clone, Debug)]
pub enum SyncEvent {
    /// A new L2 block was posted to L1.
    L1BlockPosted(Vec<L2BlockId>),

    /// Received a new L2 block from somewhere, maybe the p2p network, maybe we
    /// just made it.
    L2BlockRecv(L2BlockId),

    /// Finished executing an L2 block with a status.
    L2BlockExec(L2BlockId, bool),
}

/// Actions the consensus state machine directs the node to take to update its
/// own bookkeeping.  These should not be able to fail.
#[derive(Clone, Debug)]
pub enum SyncAction {
    /// Directs the EL engine to try to check a block ID.
    TryCheckBlock(L2BlockId),

    /// Extends our externally-facing tip to a new block ID.
    ExtendTip(L2BlockId),

    /// Reverts out externally-facing tip to a new block ID, directing the EL
    /// engine to roll back changes.
    RevertTip(L2BlockId),
}