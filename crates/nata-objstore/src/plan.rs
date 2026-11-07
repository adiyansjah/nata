//! Range planning (ADR 0011): merge needed byte ranges only within the byte-waste allowance; split what is too large.
//!
//! The rule as written: gaps up to `merge_gap` (1 MiB) are merged only when the merged response stays at most `merge_max` (64 MiB) and the
//! cumulative extra bytes (the gaps read but not needed) stay at most `waste_pct` (25 %) of the needed bytes of the scan.
//! The control is `plan_unmerged`: one request per needed range (split at `merge_max`).

use std::ops::Range;
use std::sync::Mutex;

use crate::policy::Config;
use crate::util::lock;

/// Needed and extra bytes of one scan so far; the 25 % rule is applied to the running totals.
#[derive(Debug, Default)]
pub struct Budget(Mutex<(u64, u64)>);

impl Budget {
    pub fn new() -> Budget {
        Budget::default()
    }
    pub fn totals(&self) -> (u64, u64) {
        *lock(&self.0)
    }
    /// Would adding `extra` extra bytes while also needing `need` more keep extra <= pct % of needed? If so, record both.
    fn try_take(&self, need: u64, extra: u64, pct: u64) -> bool {
        let mut g = lock(&self.0);
        let (n, e) = (g.0 + need, g.1 + extra);
        if (e as u128) * 100 <= (n as u128) * pct as u128 {
            *g = (n, e);
            true
        } else {
            false
        }
    }
    fn add_needed(&self, need: u64) {
        lock(&self.0).0 += need;
    }
}

/// One request: the byte range to fetch and which of the needed ranges (indices into the input) it serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub fetch: Range<u64>,
    pub covers: Vec<usize>,
}

fn split(r: Range<u64>, max: u64) -> impl Iterator<Item = Range<u64>> {
    let mut s = r.start;
    std::iter::from_fn(move || {
        if s >= r.end {
            return None;
        }
        let e = (s + max).min(r.end);
        let out = s..e;
        s = e;
        Some(out)
    })
}

/// Control: one request per needed range, split at `merge_max`.
pub fn plan_unmerged(needed: &[Range<u64>], cfg: &Config) -> Vec<Planned> {
    let mut out = Vec::new();
    for (i, r) in needed.iter().enumerate() {
        if r.start >= r.end {
            continue;
        }
        for piece in split(r.clone(), cfg.merge_max) {
            out.push(Planned {
                fetch: piece,
                covers: vec![i],
            });
        }
    }
    out
}

/// The wrapper's plan. Needed ranges are non-empty; they may overlap.
pub fn plan_merged(needed: &[Range<u64>], cfg: &Config, budget: &Budget) -> Vec<Planned> {
    let mut idx: Vec<usize> = (0..needed.len())
        .filter(|&i| needed[i].start < needed[i].end)
        .collect();
    idx.sort_by_key(|&i| (needed[i].start, needed[i].end));
    let mut out: Vec<Planned> = Vec::new();
    let mut cur: Option<(Range<u64>, Vec<usize>)> = None;
    let flush = |cur: &mut Option<(Range<u64>, Vec<usize>)>, out: &mut Vec<Planned>| {
        if let Some((r, covers)) = cur.take() {
            for piece in split(r, cfg.merge_max) {
                out.push(Planned {
                    fetch: piece,
                    covers: covers.clone(),
                });
            }
        }
    };
    for i in idx {
        let r = &needed[i];
        match cur.as_mut() {
            None => {
                budget.add_needed(r.end - r.start);
                cur = Some((r.clone(), vec![i]));
            }
            Some((c, covers)) => {
                let gap = r.start.saturating_sub(c.end);
                let new_end = c.end.max(r.end);
                let need = r.end - r.start.max(c.end).min(r.end); // bytes of r not already inside the group
                if gap <= cfg.merge_gap
                    && new_end - c.start <= cfg.merge_max
                    && budget.try_take(need, gap, cfg.waste_pct)
                {
                    c.end = new_end;
                    covers.push(i);
                } else {
                    flush(&mut cur, &mut out);
                    budget.add_needed(r.end - r.start);
                    cur = Some((r.clone(), vec![i]));
                }
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Rng;

    fn cfg() -> Config {
        Config::adr()
    }

    #[test]
    fn merges_small_gaps_and_keeps_big_ones_apart() {
        let b = Budget::new();
        let needed = vec![0..1000, 1100..2000, 5_000_000..5_001_000];
        let p = plan_merged(&needed, &cfg(), &b);
        assert_eq!(p.len(), 2, "{p:?}");
        assert_eq!(p[0].fetch, 0..2000);
        assert_eq!(p[0].covers, vec![0, 1]);
        assert_eq!(p[1].fetch, 5_000_000..5_001_000);
    }

    #[test]
    fn the_waste_allowance_stops_merging() {
        // needed 1000 + 1000; the gap between them is 900 (> 25 % of 2000 = 500): not merged. A gap of 400 is.
        let b = Budget::new();
        assert_eq!(plan_merged(&[0..1000, 1900..2900], &cfg(), &b).len(), 2);
        let b = Budget::new();
        assert_eq!(plan_merged(&[0..1000, 1400..2400], &cfg(), &b).len(), 1);
    }

    #[test]
    fn large_ranges_are_split_at_the_response_cap() {
        let c = Config {
            merge_max: 100,
            ..cfg()
        };
        #[allow(clippy::single_range_in_vec_init)] // one needed range of 250 bytes
        let one = [0..250];
        let p = plan_unmerged(&one, &c);
        assert_eq!(
            p.iter().map(|x| x.fetch.clone()).collect::<Vec<_>>(),
            vec![0..100, 100..200, 200..250]
        );
        let p = plan_merged(&one, &c, &Budget::new());
        assert_eq!(p.len(), 3);
    }

    /// Property: for random inputs every needed byte is covered, no response exceeds the cap, a merged group's gaps respect the gap size,
    /// and the cumulative extra bytes never exceed the allowance. Seeded, so a failure replays.
    #[test]
    fn property_cover_cap_gap_and_waste() {
        for seed in 0..300u64 {
            let mut rng = Rng::new(seed);
            let c = Config {
                merge_gap: 1 + rng.below(5000),
                merge_max: 1000 + rng.below(20_000),
                waste_pct: rng.below(60),
                ..cfg()
            };
            let n = 1 + rng.below(40) as usize;
            let mut needed = Vec::new();
            let mut pos = rng.below(100);
            for _ in 0..n {
                let len = 1 + rng.below(3000);
                needed.push(pos..pos + len);
                pos += len + rng.below(8000);
            }
            let budget = Budget::new();
            let plan = plan_merged(&needed, &c, &budget);
            let mut needed_bytes = 0u64;
            for (i, r) in needed.iter().enumerate() {
                needed_bytes += r.end - r.start;
                // covered by the union of fetches that list i
                let mut got = r.start;
                let mut pieces: Vec<&Planned> =
                    plan.iter().filter(|p| p.covers.contains(&i)).collect();
                pieces.sort_by_key(|p| p.fetch.start);
                for p in pieces {
                    if p.fetch.start <= got && p.fetch.end > got {
                        got = p.fetch.end;
                    }
                }
                assert!(
                    got >= r.end,
                    "seed {seed}: range {i} {r:?} not covered by {plan:?}"
                );
            }
            let mut fetched = 0u64;
            for p in &plan {
                assert!(
                    p.fetch.end - p.fetch.start <= c.merge_max,
                    "seed {seed}: response over the cap"
                );
                fetched += p.fetch.end - p.fetch.start;
            }
            let (n_t, e_t) = budget.totals();
            assert_eq!(n_t, needed_bytes, "seed {seed}: needed bytes");
            assert!(
                e_t * 100 <= n_t * c.waste_pct,
                "seed {seed}: extra {e_t} > {}% of {n_t}",
                c.waste_pct
            );
            assert!(
                fetched <= needed_bytes + e_t,
                "seed {seed}: fetched {fetched} > needed+extra"
            );
        }
    }
}
