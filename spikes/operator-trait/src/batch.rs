//! A deliberately tiny batch: equal-length columns of `i64`. The real engine uses Arrow
//! arrays plus a selection vector (ADR 0003); this spike only needs rows to move around.

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Batch {
    pub cols: Vec<Vec<i64>>,
}

impl Batch {
    pub fn new(ncols: usize) -> Self {
        Batch {
            cols: vec![Vec::new(); ncols],
        }
    }

    pub fn from_cols(cols: Vec<Vec<i64>>) -> Self {
        debug_assert!(cols.windows(2).all(|w| w[0].len() == w[1].len()));
        Batch { cols }
    }

    pub fn len(&self) -> usize {
        self.cols.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push_row(&mut self, row: &[i64]) {
        if self.cols.is_empty() {
            self.cols = vec![Vec::new(); row.len()];
        }
        for (c, v) in self.cols.iter_mut().zip(row) {
            c.push(*v);
        }
    }

    pub fn row(&self, i: usize) -> Vec<i64> {
        self.cols.iter().map(|c| c[i]).collect()
    }

    pub fn rows(&self) -> impl Iterator<Item = Vec<i64>> + '_ {
        (0..self.len()).map(|i| self.row(i))
    }
}
