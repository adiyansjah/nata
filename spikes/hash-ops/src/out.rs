//! Canonical results: what the engine, the hand-fused baselines and the scalar reference all produce, so
//! they can be compared digit for digit and printed the way DuckDB prints them.
use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum KVal {
    Null,
    I(i64),
    S(String),
}

/// A borrowed key value handed to visitors (no allocation per group).
#[derive(Debug, Clone, Copy)]
pub enum KRef<'a> {
    Null,
    I(i64),
    S(&'a [u8]),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GroupOut {
    pub count: u64,
    /// (sum, number of non-NULL values summed) per SUM; the sum is NULL when the second is 0.
    pub sums: Vec<(i128, u64)>,
}

#[derive(Debug, Clone, Default)]
pub struct Output {
    pub rows: BTreeMap<Vec<KVal>, GroupOut>,
    /// Decimal scale of each SUM, for printing.
    pub scales: Vec<u32>,
    pub show_count: bool,
}

impl Output {
    pub fn new(scales: Vec<u32>, show_count: bool) -> Self {
        Output { rows: BTreeMap::new(), scales, show_count }
    }

    pub fn add(&mut self, keys: &[KRef], count: u64, sums: &[(i128, u64)]) {
        let k: Vec<KVal> = keys
            .iter()
            .map(|k| match *k {
                KRef::Null => KVal::Null,
                KRef::I(v) => KVal::I(v),
                KRef::S(b) => KVal::S(String::from_utf8_lossy(b).into_owned()),
            })
            .collect();
        let prev = self.rows.insert(k.clone(), GroupOut { count, sums: sums.to_vec() });
        assert!(prev.is_none(), "duplicate group key in a result: {k:?}");
    }

    /// One line per group in DuckDB's text form, sorted bytewise (the order is not part of the result).
    pub fn lines(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .rows
            .iter()
            .map(|(k, g)| {
                let mut f: Vec<String> = k
                    .iter()
                    .map(|x| match x {
                        KVal::Null => "NULL".into(),
                        KVal::I(i) => i.to_string(),
                        KVal::S(s) => s.clone(),
                    })
                    .collect();
                if self.show_count {
                    f.push(g.count.to_string());
                }
                for (j, (s, nn)) in g.sums.iter().enumerate() {
                    f.push(if *nn == 0 { "NULL".into() } else { dec_text(*s, self.scales[j]) });
                }
                f.join("|")
            })
            .collect();
        v.sort();
        v
    }
}

pub fn dec_text(v: i128, scale: u32) -> String {
    if scale == 0 {
        return v.to_string();
    }
    let p = 10i128.pow(scale);
    format!("{}{}.{:0w$}", if v < 0 { "-" } else { "" }, v.abs() / p, v.abs() % p, w = scale as usize)
}

/// Order-independent digest of a result, computed without building a map: the wrapping sum of a hash of
/// every (group key, count, sums).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Digest {
    pub groups: u64,
    pub sum: u64,
}

impl Digest {
    pub fn add(&mut self, keys: &[KRef], count: u64, sums: &[(i128, u64)]) {
        let mut h = foldhash::fast::FixedState::with_seed(7).build_hasher();
        for k in keys {
            match *k {
                KRef::Null => h.write_u8(0),
                KRef::I(v) => {
                    h.write_u8(1);
                    h.write_i64(v)
                }
                KRef::S(b) => {
                    h.write_u8(2);
                    h.write_usize(b.len());
                    h.write(b)
                }
            }
        }
        h.write_u64(count);
        for (s, nn) in sums {
            h.write_i128(*s);
            h.write_u64(*nn);
        }
        self.groups += 1;
        self.sum = self.sum.wrapping_add(h.finish());
    }
}

/// Anything that can enumerate its groups: the engine's merged tables, the fused baselines' tables.
pub trait Groups {
    fn for_each_group(&self, f: &mut dyn FnMut(&[KRef], u64, &[(i128, u64)]));
}

pub fn digest_of(g: &dyn Groups) -> Digest {
    let mut d = Digest::default();
    g.for_each_group(&mut |k, c, s| d.add(k, c, s));
    d
}

pub fn output_of(g: &dyn Groups, scales: Vec<u32>, show_count: bool) -> Output {
    let mut o = Output::new(scales, show_count);
    g.for_each_group(&mut |k, c, s| o.add(k, c, s));
    o
}
