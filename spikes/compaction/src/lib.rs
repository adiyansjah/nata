//! THROWAWAY SPIKE (ADR 0003, verification-plan item 18): the cost of compacting a batch (materialising only the selected
//! rows with arrow `take` / `filter`) before a stock arrow kernel, against carrying the selection vector into our own
//! selection-aware kernels. See bench/results/20261104-compaction.md.
pub mod arms;
pub mod data;
pub mod grid;
pub mod harness;
pub mod kernels;
pub mod reference;
