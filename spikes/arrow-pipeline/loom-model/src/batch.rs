//! Stand-in for `spikes/arrow-pipeline/src/batch.rs`: i64 values with an optional selection vector, and a counter of
//! live batches so a test can see whether queued morsels are released (a leaked channel keeps them alive).
//! Same method names and semantics as the Arrow batch for everything the included scheduler/exchange/limit files use.
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

/// Counts the batches alive (created or cloned minus dropped). A std atomic on purpose: it is bookkeeping of the test,
/// not part of the protocol, so it adds no interleavings.
#[derive(Default)]
pub struct Live(pub AtomicUsize);

impl Live {
    pub fn get(&self) -> usize {
        self.0.load(SeqCst)
    }
}

pub struct Batch {
    vals: Arc<Vec<i64>>,
    sel: Option<Arc<Vec<u32>>>,
    live: Option<Arc<Live>>,
}

impl Default for Batch {
    fn default() -> Self {
        Batch { vals: Arc::new(Vec::new()), sel: None, live: None }
    }
}

impl Clone for Batch {
    fn clone(&self) -> Self {
        Batch::tracked(self.vals.clone(), self.sel.clone(), self.live.clone())
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        if let Some(l) = &self.live {
            let before = l.0.fetch_sub(1, SeqCst);
            assert!(before >= 1, "live-batch counter underflow");
        }
    }
}

#[derive(Debug)]
pub struct CompactError;

impl fmt::Display for CompactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("compact failed")
    }
}

impl Batch {
    fn tracked(vals: Arc<Vec<i64>>, sel: Option<Arc<Vec<u32>>>, live: Option<Arc<Live>>) -> Batch {
        if let Some(l) = &live {
            l.0.fetch_add(1, SeqCst);
        }
        Batch { vals, sel, live }
    }

    /// A batch of `vals` (no selection), counted in `live`.
    pub fn new(vals: Vec<i64>, live: &Arc<Live>) -> Batch {
        Batch::tracked(Arc::new(vals), None, Some(Arc::clone(live)))
    }

    /// Physical rows.
    pub fn rows(&self) -> usize {
        self.vals.len()
    }

    /// Live rows (after the selection).
    pub fn len(&self) -> usize {
        match &self.sel {
            Some(s) => s.len(),
            None => self.rows(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn sel(&self) -> Option<&[u32]> {
        self.sel.as_ref().map(|s| s.as_slice())
    }

    pub fn with_sel(&self, sel: Vec<u32>) -> Batch {
        Batch::tracked(self.vals.clone(), Some(Arc::new(sel)), self.live.clone())
    }

    /// The values of the live rows, in order.
    pub fn values(&self) -> Vec<i64> {
        match &self.sel {
            Some(s) => s.iter().map(|&i| self.vals[i as usize]).collect(),
            None => self.vals.as_ref().clone(),
        }
    }

    /// A copy without selection vector (what the exchange producer does before a batch enters the channel).
    pub fn compact(&self) -> Result<Batch, CompactError> {
        Ok(Batch::tracked(Arc::new(self.values()), None, self.live.clone()))
    }
}
