//! Tracker for keeping track of the tree of unfinalized blocks.

use std::collections::*;

use alpen_vertex_primitives::buf::Buf32;
use alpen_vertex_state::block::{L2BlockHeader, L2BlockId};

use crate::errors::ChainTipError;

/// Entry in block tracker table we use to relate a block with its immediate
/// relatives.
struct BlockEntry {
    parent: L2BlockId,
    children: HashSet<L2BlockId>,
}

/// Tracks the unfinalized block tree on top of the finalized tip.
pub struct UnfinalizedBlockTracker {
    /// Block that we treat as a base that all of the other blocks that we're
    /// considering uses.
    finalized_tip: L2BlockId,

    /// Table of pending blocks near the tip of the block tree.
    pending_table: HashMap<L2BlockId, BlockEntry>,

    /// Unfinalized chain tips.  This also includes the finalized tip if there's
    /// no pending blocks.
    unfinalized_tips: HashSet<L2BlockId>,
}

impl UnfinalizedBlockTracker {
    /// Creates a new tracker with just a finalized tip and no pending blocks.
    pub fn new_empty(finalized_tip: L2BlockId) -> Self {
        let mut pending_tbl = HashMap::new();
        pending_tbl.insert(
            finalized_tip,
            BlockEntry {
                parent: L2BlockId::from(Buf32::zero()),
                children: HashSet::new(),
            },
        );

        let mut unf_tips = HashSet::new();
        unf_tips.insert(finalized_tip);
        Self {
            finalized_tip,
            pending_table: pending_tbl,
            unfinalized_tips: unf_tips,
        }
    }

    /// Returns the "fianlized tip", which is the base of the unfinalized tree.
    pub fn finalized_tip(&self) -> &L2BlockId {
        &self.finalized_tip
    }

    /// Gets the parent of a block from within the tree.  Returns `None` if the
    /// block or its parent isn't in the tree.  Returns `None` for the finalized
    /// tip block, since its parent isn't in the tree.
    pub fn get_parent(&self, id: &L2BlockId) -> Option<&L2BlockId> {
        if *id == self.finalized_tip {
            return None;
        }
        self.pending_table.get(id).map(|ent| &ent.parent)
    }

    /// Returns an iterator over the chain tips.
    pub fn chain_tips_iter(&self) -> impl Iterator<Item = &L2BlockId> {
        self.unfinalized_tips.iter()
    }

    /// Checks if the block is traceable all the way back to the finalized tip.
    fn sanity_check_parent_seq(&self, blkid: &L2BlockId) -> bool {
        if *blkid == self.finalized_tip {
            return true;
        }

        if let Some(ent) = self.pending_table.get(blkid) {
            self.sanity_check_parent_seq(&ent.parent)
        } else {
            false
        }
    }

    /// Tries to attach a block to the tree.  Does not verify the header
    /// corresponds to the given blockid.
    ///
    /// Returns if this new block forks off and creates a new unfinalized tip
    /// block.
    // TODO do a `SealedL2BlockHeader` thing that includes the blkid
    pub fn attach_block(
        &mut self,
        blkid: L2BlockId,
        header: &L2BlockHeader,
    ) -> Result<bool, ChainTipError> {
        if self.pending_table.contains_key(&blkid) {
            return Err(ChainTipError::BlockAlreadyAttached(blkid));
        }

        let parent_blkid = header.parent();

        if let Some(parent_ent) = self.pending_table.get_mut(parent_blkid) {
            parent_ent.children.insert(blkid);
        } else {
            return Err(ChainTipError::AttachMissingParent(blkid, *header.parent()));
        }

        let ent = BlockEntry {
            parent: *header.parent(),
            children: HashSet::new(),
        };

        self.pending_table.insert(blkid, ent);

        // Also update the tips table, removing the parent if it's there.
        let did_replace = self.unfinalized_tips.remove(parent_blkid);
        self.unfinalized_tips.insert(blkid);

        Ok(!did_replace)
    }

    /// Updates the finalized block tip, returning a report that includes the
    /// precise blocks that were finalized transatively and any blocks on
    /// competing chains that were rejected.
    pub fn update_finalized_tip(
        &mut self,
        blkid: &L2BlockId,
    ) -> Result<FinalizeReport, ChainTipError> {
        // Sanity check the block so we know it's here.
        if !self.sanity_check_parent_seq(blkid) {
            return Err(ChainTipError::MissingBlock(*blkid));
        }

        let mut path = vec![];
        let mut to_evict = Vec::new();
        let mut at = *blkid;

        // Walk down to the current finalized tip and put everything in the
        // eviction table.
        while at != self.finalized_tip {
            path.push(at);

            // Get the parent of the block we're at, add all of the children
            // other than the one we're at to the eviction table.
            let ent = self.pending_table.get(&at).unwrap();
            for ch in &ent.children {
                if *ch != at {
                    to_evict.push(*ch);
                }
            }

            at = ent.parent;
        }

        // Now actually go through and evict all the blocks we said we were
        // going to, adding more on as we add them.
        let mut evicted = Vec::new();
        while !to_evict.is_empty() {
            let evicting = to_evict.pop().unwrap();
            let ent = self
                .pending_table
                .remove(&evicting)
                .expect("chaintip: evicting dangling child ref");
            self.unfinalized_tips.remove(&evicting);

            to_evict.extend(ent.children.into_iter());
            evicted.push(evicting);
        }

        // And also remove blocks that we're finalizing, *except* the new
        // finalized tip.
        assert!(self.pending_table.remove(&self.finalized_tip).is_some());
        for pblk in &path {
            if pblk != blkid {
                assert!(self.pending_table.remove(pblk).is_some());
            }
        }

        // Just update the finalized tip now.
        let old_tip = self.finalized_tip;
        self.finalized_tip = *blkid;

        // Sanity check and construct the report.
        assert!(!path.is_empty(), "chaintip: somehow finalized no blocks");
        Ok(FinalizeReport {
            prev_tip: old_tip,
            finalized: path,
            rejected: evicted,
        })
    }

    #[cfg(test)]
    pub fn unchecked_set_finalized_tip(&mut self, id: L2BlockId) {
        self.finalized_tip = id;
    }

    #[cfg(test)]
    pub fn insert_fake_block(&mut self, id: L2BlockId, parent: L2BlockId) {
        let ent = BlockEntry {
            parent,
            children: HashSet::new(),
        };

        self.pending_table.insert(id, ent);
    }
}

/// Report of blocks that we finalized when finalizing a new tip and blocks that
/// we've permanently rejected.
#[derive(Clone, Debug)]
pub struct FinalizeReport {
    /// Previous tip.
    prev_tip: L2BlockId,

    /// Block we've newly finalized.  The first one of this
    finalized: Vec<L2BlockId>,

    /// Any blocks that were on competing chains than the one we finalized.
    rejected: Vec<L2BlockId>,
}

impl FinalizeReport {
    /// Returns the blkid that was the previously finalized tip.  It's still
    /// finalized, but there's newer blocks that are also finalized now.
    pub fn prev_tip(&self) -> &L2BlockId {
        &self.prev_tip
    }

    /// The new chain tip that's finalized now.
    pub fn new_tip(&self) -> &L2BlockId {
        if self.finalized.is_empty() {
            &self.prev_tip
        } else {
            &self.finalized[0]
        }
    }

    /// Returns a slice of the blkids that were rejected.
    pub fn rejected(&self) -> &[L2BlockId] {
        &self.rejected
    }

    /// Returns an iterator over the blkids that were rejected.
    pub fn rejected_iter(&self) -> impl Iterator<Item = &L2BlockId> {
        self.rejected.iter()
    }
}

// TODO unit tests