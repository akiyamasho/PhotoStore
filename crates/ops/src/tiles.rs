//! Tile references owned by history snapshots. The snapshots keep these addresses alive;
//! COW edits cannot change a retained tile in place. Count occurrences (including aliases
//! within one document), so removing a state releases a tile only after its last reference.
use photostore_doc::{Document, Surface};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub(crate) struct Tiles {
    refs: HashMap<usize, (u128, usize)>,
    bytes: u128,
}

pub(crate) fn surfaces(doc: &Document, mut visit: impl FnMut(&Surface)) {
    for (_, _, layer) in doc.walk() {
        if let Some(surface) = layer.surface() {
            visit(surface);
        }
        if let Some(mask) = &layer.mask {
            visit(&mask.surface);
        }
    }
    for channel in doc.channels.iter().chain(&doc.quick_mask) {
        visit(&channel.surface);
    }
}

/// Stable live allocation identities for pixels and shared authored frame metadata.
pub(crate) fn allocations(doc:&Document,mut visit:impl FnMut(usize,usize)) {
    surfaces(doc,|surface|{for (_,tile) in surface.tiles(){visit(Arc::as_ptr(tile) as usize,tile.bytes().len());}});
    if let Some(timeline)=&doc.timeline {for bank in [&timeline.frame_states,&timeline.property_frames] {
        if !bank.is_empty(){visit(bank.allocation_key(),bank.retained_bytes());}
    }}
}

impl Tiles {
    pub(crate) fn add(&mut self, doc: &Document) {
        allocations(doc,|key,bytes|{
            let count=self.refs.entry(key).or_insert_with(||{self.bytes+=bytes as u128;(0,bytes)});
            count.0+=1;
        });
    }

    pub(crate) fn remove(&mut self, doc: &Document) {
        allocations(doc,|key,_|{
            if let Some(count)=self.refs.get_mut(&key){
                count.0-=1;
                if count.0==0 {self.bytes-=count.1 as u128;self.refs.remove(&key);}
            }
        });
    }

    /// Retained bytes are incremental; only current tiles need examining below the limit.
    /// Current can change without recording (coalescing), and tile transfers can carry a
    /// different byte length, so a tile-count estimate must not underestimate its memory.
    pub(crate) fn fits(&self, current: &Document, budget: usize) -> bool {
        self.bytes <= budget as u128 && self.bytes_with(current) <= budget as u128
    }

    pub(crate) fn bytes_with(&self, current: &Document) -> u128 {
        let mut total = self.bytes;
        let mut seen = HashSet::new();
        allocations(current,|key,bytes|{
            if !self.refs.contains_key(&key) && seen.insert(key){total+=bytes as u128;}
        });
        total
    }
}
