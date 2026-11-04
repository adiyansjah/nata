//! Hand-fused baselines for each query: specialised loops over the same batches that use hashbrown tables
//! directly, with no generic stages, no selection vector, no NULL handling (the data has none; asserted)
//! and no checked-kernel plumbing beyond the same overflow flag. They stand in for what a query compiler
//! could emit. Same algorithms as the engine where it matters for a fair comparison: thread-local
//! partitioned tables merged per partition in parallel; a partitioned parallel join build.
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use arrow::array::{Array, AsArray, RecordBatch};
use arrow::datatypes::{Date32Type, Decimal64Type, Int32Type, Int64Type};
use hashbrown::HashTable;

use crate::engine::Visit;
use crate::kernels::{EngineError, R, overflow};
use crate::keys::{PARTS, hash_bytes, mix, part_of};
use crate::out::{Groups, KRef};
use crate::queries::*;

fn no_nulls(a: &dyn Array) {
    assert_eq!(a.null_count(), 0, "the fused baseline does not handle NULLs");
}

fn col<'a>(rb: &'a RecordBatch, n: &str) -> &'a dyn Array {
    let a = rb.column(rb.schema().index_of(n).expect("column")).as_ref();
    no_nulls(a);
    a
}
fn i64s<'a>(rb: &'a RecordBatch, n: &str) -> &'a [i64] {
    let a = col(rb, n);
    match a.data_type() {
        arrow::datatypes::DataType::Int64 => a.as_primitive::<Int64Type>().values(),
        _ => a.as_primitive::<Decimal64Type>().values(),
    }
}
fn dates<'a>(rb: &'a RecordBatch, n: &str) -> &'a [i32] {
    col(rb, n).as_primitive::<Date32Type>().values()
}
fn i32s<'a>(rb: &'a RecordBatch, n: &str) -> &'a [i32] {
    col(rb, n).as_primitive::<Int32Type>().values()
}
/// (dictionary key per row, the dictionary's values as byte slices)
fn dict<'a>(rb: &'a RecordBatch, n: &str) -> (&'a [i32], Vec<&'a [u8]>) {
    let d = col(rb, n).as_dictionary::<Int32Type>();
    let v = d.values().as_string::<i32>();
    (d.keys().values(), (0..v.len()).map(|j| v.value(j).as_bytes()).collect())
}

/// Morsel-driven workers, each with its own state; returns the states in worker order.
fn workers<S: Send>(threads: usize, morsels: usize, visit: Visit, mk: &(dyn Fn() -> S + Sync), f: &(dyn Fn(&mut S, &RecordBatch) + Sync)) -> Vec<S> {
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut st = mk();
                    loop {
                        let m = next.fetch_add(1, Relaxed);
                        if m >= morsels {
                            break;
                        }
                        visit(m, &mut |rb| f(&mut st, rb));
                    }
                    st
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("worker")).collect()
    })
}

/// Run `f(p)` for every partition on `threads` workers; results in partition order.
fn per_partition<T: Send>(threads: usize, f: &(dyn Fn(usize) -> T + Sync)) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let mut all: Vec<(usize, T)> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let p = next.fetch_add(1, Relaxed);
                        if p >= PARTS {
                            break;
                        }
                        mine.push((p, f(p)));
                    }
                    mine
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().expect("partition worker")).collect()
    });
    all.sort_by_key(|(p, _)| *p);
    all.into_iter().map(|(_, t)| t).collect()
}

// ---------------------------------------------------------------- aggregates

#[derive(Clone, Copy, Default)]
struct Acc {
    count: u64,
    s: [i128; 2],
}

impl Acc {
    fn add(&mut self, o: &Acc) {
        self.count += o.count;
        self.s[0] += o.s[0];
        self.s[1] += o.s[1];
    }
}

/// Per-thread aggregate state for an integer-like key: PARTS hash tables.
struct IntAgg<K> {
    parts: Vec<HashTable<(K, Acc)>>,
    bad: bool,
}

impl<K> IntAgg<K> {
    fn new() -> Self {
        IntAgg { parts: (0..PARTS).map(|_| HashTable::new()).collect(), bad: false }
    }
}

#[inline(always)]
fn upsert<K: Copy + Eq>(parts: &mut [HashTable<(K, Acc)>], h: u64, k: K, hf: impl Fn(&K) -> u64) -> &mut Acc {
    let t = &mut parts[part_of(h)];
    let e = t.entry(h, |e| e.0 == k, |e| hf(&e.0)).or_insert_with(|| (k, Acc::default()));
    &mut e.into_mut().1
}

fn merge_int<K: Copy + Eq + Send + Sync>(locals: Vec<IntAgg<K>>, threads: usize, hf: &(dyn Fn(&K) -> u64 + Sync)) -> R<Vec<HashTable<(K, Acc)>>> {
    if locals.iter().any(|l| l.bad) {
        return overflow("multiplication");
    }
    let mut locals: Vec<Vec<HashTable<(K, Acc)>>> = locals.into_iter().map(|l| l.parts).collect();
    if locals.len() == 1 {
        return Ok(locals.pop().expect("one"));
    }
    let locals = &locals;
    Ok(per_partition(threads, &|p| {
        let mut dst: HashTable<(K, Acc)> = HashTable::new();
        for l in locals {
            for e in l[p].iter() {
                dst.entry(hf(&e.0), |d| d.0 == e.0, |d| hf(&d.0)).or_insert_with(|| (e.0, Acc::default())).into_mut().1.add(&e.1);
            }
        }
        dst
    }))
}

struct IntResult<K> {
    parts: Vec<HashTable<(K, Acc)>>,
    n_sums: usize,
    key: fn(&K) -> [KRefOwned; 3],
    nkeys: usize,
}

#[derive(Clone, Copy)]
enum KRefOwned {
    I(i64),
    None,
}

impl<K> Groups for IntResult<K> {
    fn for_each_group(&self, f: &mut dyn FnMut(&[KRef], u64, &[(i128, u64)])) {
        for t in &self.parts {
            for (k, a) in t.iter() {
                let ks = (self.key)(k);
                let kr: Vec<KRef> = ks[..self.nkeys].iter().map(|x| match x { KRefOwned::I(v) => KRef::I(*v), KRefOwned::None => KRef::Null }).collect();
                let sums: Vec<(i128, u64)> = (0..self.n_sums).map(|j| (a.s[j], a.count)).collect();
                f(&kr, a.count, &sums);
            }
        }
    }
}

fn hf_i64(k: &i64) -> u64 {
    mix(*k as u64)
}

pub fn fused_agg_orderkey(inp: &Inputs, threads: usize) -> R<Box<dyn Groups>> {
    let locals = workers(threads, inp.lineitem.0, inp.lineitem.1, &IntAgg::<i64>::new, &|st, rb| {
        let (key, price, disc, date) = (i64s(rb, "l_orderkey"), i64s(rb, "l_extendedprice"), i64s(rb, "l_discount"), dates(rb, "l_shipdate"));
        for i in 0..rb.num_rows() {
            if date[i] <= CUTOFF {
                let (t, o1) = 100i64.overflowing_sub(disc[i]);
                let (m, o2) = price[i].overflowing_mul(t);
                st.bad |= o1 | o2;
                let a = upsert(&mut st.parts, hf_i64(&key[i]), key[i], hf_i64);
                a.count += 1;
                a.s[0] += price[i] as i128;
                a.s[1] += m as i128;
            }
        }
    });
    let parts = merge_int(locals, threads, &hf_i64)?;
    Ok(Box::new(IntResult { parts, n_sums: 2, key: |k| [KRefOwned::I(*k), KRefOwned::None, KRefOwned::None], nkeys: 1 }))
}

pub fn fused_agg_partkey(inp: &Inputs, threads: usize) -> R<Box<dyn Groups>> {
    let locals = workers(threads, inp.lineitem.0, inp.lineitem.1, &IntAgg::<i64>::new, &|st, rb| {
        let (key, qty, price, disc, date) = (i64s(rb, "l_partkey"), i64s(rb, "l_quantity"), i64s(rb, "l_extendedprice"), i64s(rb, "l_discount"), dates(rb, "l_shipdate"));
        for i in 0..rb.num_rows() {
            if date[i] <= CUTOFF {
                let (t, o1) = 100i64.overflowing_sub(disc[i]);
                let (m, o2) = price[i].overflowing_mul(t);
                st.bad |= o1 | o2;
                let a = upsert(&mut st.parts, hf_i64(&key[i]), key[i], hf_i64);
                a.count += 1;
                a.s[0] += qty[i] as i128;
                a.s[1] += m as i128;
            }
        }
    });
    let parts = merge_int(locals, threads, &hf_i64)?;
    Ok(Box::new(IntResult { parts, n_sums: 2, key: |k| [KRefOwned::I(*k), KRefOwned::None, KRefOwned::None], nkeys: 1 }))
}

const MODES: [&str; 7] = ["AIR", "FOB", "MAIL", "RAIL", "REG AIR", "SHIP", "TRUCK"];

fn mode_code(b: &[u8]) -> i64 {
    MODES.iter().position(|m| m.as_bytes() == b).expect("known ship mode") as i64
}

pub fn fused_agg_modedate(inp: &Inputs, threads: usize) -> R<Box<dyn Groups>> {
    // Key packed as (mode code << 32) | date.
    let hf = |k: &i64| mix(*k as u64);
    let locals = workers(threads, inp.lineitem.0, inp.lineitem.1, &IntAgg::<i64>::new, &|st, rb| {
        let (mode_keys, mode_vals) = dict(rb, "l_shipmode");
        let codes: Vec<i64> = mode_vals.iter().map(|b| mode_code(b)).collect();
        let (date, disc, qty, price) = (dates(rb, "l_shipdate"), i64s(rb, "l_discount"), i64s(rb, "l_quantity"), i64s(rb, "l_extendedprice"));
        for i in 0..rb.num_rows() {
            if (2..=8).contains(&disc[i]) {
                let k = (codes[mode_keys[i] as usize] << 32) | (date[i] as u32 as i64);
                let a = upsert(&mut st.parts, hf(&k), k, hf);
                a.count += 1;
                a.s[0] += qty[i] as i128;
                a.s[1] += price[i] as i128;
            }
        }
    });
    let parts = merge_int(locals, threads, &hf)?;
    struct R_ {
        parts: Vec<HashTable<(i64, Acc)>>,
    }
    impl Groups for R_ {
        fn for_each_group(&self, f: &mut dyn FnMut(&[KRef], u64, &[(i128, u64)])) {
            for t in &self.parts {
                for (k, a) in t.iter() {
                    f(&[KRef::S(MODES[(*k >> 32) as usize].as_bytes()), KRef::I((*k as u32 as i32) as i64)], a.count, &[(a.s[0], a.count), (a.s[1], a.count)]);
                }
            }
        }
    }
    Ok(Box::new(R_ { parts }))
}

/// A string-keyed table: entries (offset, length, acc) into a per-partition arena.
struct StrPart {
    table: HashTable<(u32, u32, Acc)>,
    arena: Vec<u8>,
}

impl StrPart {
    fn new() -> Self {
        StrPart { table: HashTable::new(), arena: Vec::new() }
    }
    #[inline(always)]
    fn upsert(&mut self, h: u64, key: &[u8]) -> &mut Acc {
        let arena = &mut self.arena;
        let e = self.table.entry(
            h,
            |e| &arena[e.0 as usize..(e.0 + e.1) as usize] == key,
            |e| hash_bytes(&arena[e.0 as usize..(e.0 + e.1) as usize]),
        );
        match e {
            hashbrown::hash_table::Entry::Occupied(o) => &mut o.into_mut().2,
            hashbrown::hash_table::Entry::Vacant(v) => {
                let off = arena.len() as u32;
                arena.extend_from_slice(key);
                &mut v.insert((off, key.len() as u32, Acc::default())).into_mut().2
            }
        }
    }
}

pub fn fused_agg_comment(inp: &Inputs, threads: usize) -> R<Box<dyn Groups>> {
    struct St {
        parts: Vec<StrPart>,
    }
    let locals = workers(threads, inp.orders.0, inp.orders.1, &|| St { parts: (0..PARTS).map(|_| StrPart::new()).collect() }, &|st, rb| {
        let s = col(rb, "o_comment").as_string::<i32>();
        let (price, date) = (i64s(rb, "o_totalprice"), dates(rb, "o_orderdate"));
        for i in 0..rb.num_rows() {
            if date[i] <= CUTOFF {
                let k = s.value(i).as_bytes();
                let h = hash_bytes(k);
                let a = st.parts[part_of(h)].upsert(h, k);
                a.count += 1;
                a.s[0] += price[i] as i128;
            }
        }
    });
    let parts: Vec<StrPart> = if locals.len() == 1 {
        locals.into_iter().next().expect("one").parts
    } else {
        let locals = &locals;
        per_partition(threads, &|p| {
            let mut dst = StrPart::new();
            for l in locals {
                let src = &l.parts[p];
                for e in src.table.iter() {
                    let k = &src.arena[e.0 as usize..(e.0 + e.1) as usize];
                    dst.upsert(hash_bytes(k), k).add(&e.2);
                }
            }
            dst
        })
    };
    struct R_(Vec<StrPart>);
    impl Groups for R_ {
        fn for_each_group(&self, f: &mut dyn FnMut(&[KRef], u64, &[(i128, u64)])) {
            for p in &self.0 {
                for e in p.table.iter() {
                    f(&[KRef::S(&p.arena[e.0 as usize..(e.0 + e.1) as usize])], e.2.count, &[(e.2.s[0], e.2.count)]);
                }
            }
        }
    }
    Ok(Box::new(R_(parts)))
}

// ---------------------------------------------------------------- joins

/// Scattered build rows: [worker][partition] -> rows.
type Scatter<T> = Vec<Vec<Vec<T>>>;

fn scatter_state<T>() -> Vec<Vec<T>> {
    (0..PARTS).map(|_| Vec::new()).collect()
}

/// A join table over unique or duplicate keys: per partition, key -> head of a chain; `next` links rows.
struct Chain<P> {
    table: HashTable<(i64, u32)>,
    rows: Vec<(i64, P)>,
    next: Vec<u32>,
}

const END: u32 = u32::MAX;

fn build_chains<P: Copy + Send + Sync>(sc: &Scatter<(i64, P)>, threads: usize) -> Vec<Chain<P>> {
    per_partition(threads, &|p| {
        let rows: Vec<(i64, P)> = sc.iter().flat_map(|w| w[p].iter().copied()).collect();
        let mut table: HashTable<(i64, u32)> = HashTable::with_capacity(rows.len());
        let mut next = vec![END; rows.len()];
        for (l, r) in rows.iter().enumerate() {
            let h = hf_i64(&r.0);
            match table.find_mut(h, |e| e.0 == r.0) {
                Some(e) => {
                    next[l] = e.1;
                    e.1 = l as u32;
                }
                None => {
                    table.insert_unique(h, (r.0, l as u32), |e| hf_i64(&e.0));
                }
            }
        }
        Chain { table, rows, next }
    })
}

#[inline(always)]
fn chain_head<P>(chains: &[Chain<P>], k: i64) -> (&Chain<P>, u32) {
    let h = hf_i64(&k);
    let c = &chains[part_of(h)];
    (c, c.table.find(h, |e| e.0 == k).map_or(END, |e| e.1))
}

pub fn fused_q3(inp: &Inputs, threads: usize) -> R<Box<dyn Groups>> {
    // 1. customers in BUILDING -> a set of keys.
    let sc1: Scatter<(i64, ())> = workers(threads, inp.customer.0, inp.customer.1, &scatter_state, &|st, rb| {
        let (keys, vals) = dict(rb, "c_mktsegment");
        let ok: Vec<bool> = vals.iter().map(|v| *v == b"BUILDING").collect();
        let ck = i64s(rb, "c_custkey");
        for i in 0..rb.num_rows() {
            if ok[keys[i] as usize] {
                st[part_of(hf_i64(&ck[i]))].push((ck[i], ()));
            }
        }
    });
    let t1 = build_chains(&sc1, threads);
    // 2. orders before 1995-03-15 whose customer is in the set -> keyed by order key, payload (date, priority).
    let sc2: Scatter<(i64, (i32, i32))> = workers(threads, inp.orders.0, inp.orders.1, &scatter_state, &|st, rb| {
        let (ok, ck, od, sp) = (i64s(rb, "o_orderkey"), i64s(rb, "o_custkey"), dates(rb, "o_orderdate"), i32s(rb, "o_shippriority"));
        for i in 0..rb.num_rows() {
            if od[i] < MAR15 && chain_head(&t1, ck[i]).1 != END {
                st[part_of(hf_i64(&ok[i]))].push((ok[i], (od[i], sp[i])));
            }
        }
    });
    drop(t1);
    let t2 = build_chains(&sc2, threads);
    drop(sc2);
    // 3. lineitem after the date probes it; group by (order key, date, priority).
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct K(i64, i32, i32);
    let hk = |k: &K| mix(mix(k.0 as u64) ^ ((k.1 as u32 as u64) << 32 | k.2 as u32 as u64));
    struct St {
        parts: Vec<HashTable<(K, Acc)>>,
        bad: bool,
    }
    let locals = workers(threads, inp.lineitem.0, inp.lineitem.1, &|| St { parts: (0..PARTS).map(|_| HashTable::new()).collect(), bad: false }, &|st, rb| {
        let (lk, price, disc, sd) = (i64s(rb, "l_orderkey"), i64s(rb, "l_extendedprice"), i64s(rb, "l_discount"), dates(rb, "l_shipdate"));
        for i in 0..rb.num_rows() {
            if sd[i] > MAR15 {
                let (c, mut l) = chain_head(&t2, lk[i]);
                while l != END {
                    let (_, (od, sp)) = c.rows[l as usize];
                    let (t, o1) = 100i64.overflowing_sub(disc[i]);
                    let (m, o2) = price[i].overflowing_mul(t);
                    st.bad |= o1 | o2;
                    let k = K(lk[i], od, sp);
                    let h = hk(&k);
                    let e = st.parts[part_of(h)].entry(h, |e| e.0 == k, |e| hk(&e.0)).or_insert_with(|| (k, Acc::default())).into_mut();
                    e.1.count += 1;
                    e.1.s[0] += m as i128;
                    l = c.next[l as usize];
                }
            }
        }
    });
    if locals.iter().any(|l| l.bad) {
        return overflow("multiplication");
    }
    let mut locals: Vec<Vec<HashTable<(K, Acc)>>> = locals.into_iter().map(|l| l.parts).collect();
    let parts = if locals.len() == 1 {
        locals.pop().expect("one")
    } else {
        let locals = &locals;
        per_partition(threads, &|p| {
            let mut dst: HashTable<(K, Acc)> = HashTable::new();
            for l in locals {
                for e in l[p].iter() {
                    dst.entry(hk(&e.0), |d| d.0 == e.0, |d| hk(&d.0)).or_insert_with(|| (e.0, Acc::default())).into_mut().1.add(&e.1);
                }
            }
            dst
        })
    };
    struct R_(Vec<HashTable<(K, Acc)>>);
    impl Groups for R_ {
        fn for_each_group(&self, f: &mut dyn FnMut(&[KRef], u64, &[(i128, u64)])) {
            for t in &self.0 {
                for (k, a) in t.iter() {
                    f(&[KRef::I(k.0), KRef::I(k.1 as i64), KRef::I(k.2 as i64)], a.count, &[(a.s[0], a.count)]);
                }
            }
        }
    }
    Ok(Box::new(R_(parts)))
}

pub fn fused_q12(inp: &Inputs, threads: usize) -> R<Box<dyn Groups>> {
    let sc: Scatter<(i64, u8)> = workers(threads, inp.lineitem.0, inp.lineitem.1, &scatter_state, &|st, rb| {
        let (mk, mv) = dict(rb, "l_shipmode");
        let ok: Vec<Option<u8>> = mv.iter().map(|v| if *v == b"MAIL" || *v == b"SHIP" { Some(mode_code(v) as u8) } else { None }).collect();
        let (lk, cd, rd, sd) = (i64s(rb, "l_orderkey"), dates(rb, "l_commitdate"), dates(rb, "l_receiptdate"), dates(rb, "l_shipdate"));
        for i in 0..rb.num_rows() {
            if let Some(code) = ok[mk[i] as usize]
                && cd[i] < rd[i]
                && sd[i] < cd[i]
                && rd[i] >= Y1994.0
                && rd[i] <= Y1994.1
            {
                st[part_of(hf_i64(&lk[i]))].push((lk[i], code));
            }
        }
    });
    let t = build_chains(&sc, threads);
    drop(sc);
    let locals = workers(threads, inp.orders.0, inp.orders.1, &|| [[0u64; 2]; 8], &|st, rb| {
        let (pk, pv) = dict(rb, "o_orderpriority");
        let high: Vec<usize> = pv.iter().map(|v| (*v == b"1-URGENT" || *v == b"2-HIGH") as usize).collect();
        let ok = i64s(rb, "o_orderkey");
        for i in 0..rb.num_rows() {
            let (c, mut l) = chain_head(&t, ok[i]);
            while l != END {
                let code = c.rows[l as usize].1 as usize;
                st[code][1 - high[pk[i] as usize]] += 1;
                l = c.next[l as usize];
            }
        }
    });
    let mut tot = [[0u64; 2]; 8];
    for l in &locals {
        for c in 0..8 {
            tot[c][0] += l[c][0];
            tot[c][1] += l[c][1];
        }
    }
    struct R_([[u64; 2]; 8]);
    impl Groups for R_ {
        fn for_each_group(&self, f: &mut dyn FnMut(&[KRef], u64, &[(i128, u64)])) {
            for (c, a) in self.0.iter().enumerate() {
                if a[0] + a[1] > 0 {
                    f(&[KRef::S(MODES[c].as_bytes())], a[0] + a[1], &[(a[0] as i128, a[0] + a[1]), (a[1] as i128, a[0] + a[1])]);
                }
            }
        }
    }
    Ok(Box::new(R_(tot)))
}

pub fn fused_j3(inp: &Inputs, threads: usize) -> R<Box<dyn Groups>> {
    let sc: Scatter<(i64, (u8, i64))> = workers(threads, inp.lineitem.0, inp.lineitem.1, &scatter_state, &|st, rb| {
        let (mk, mv) = dict(rb, "l_shipmode");
        let codes: Vec<u8> = mv.iter().map(|v| mode_code(v) as u8).collect();
        let (lk, price) = (i64s(rb, "l_orderkey"), i64s(rb, "l_extendedprice"));
        for i in 0..rb.num_rows() {
            st[part_of(hf_i64(&lk[i]))].push((lk[i], (codes[mk[i] as usize], price[i])));
        }
    });
    let t = build_chains(&sc, threads);
    drop(sc);
    const PRIOS: [&str; 5] = ["1-URGENT", "2-HIGH", "3-MEDIUM", "4-NOT SPECIFIED", "5-LOW"];
    let locals = workers(threads, inp.orders.0, inp.orders.1, &|| [(0u64, 0i128); 35], &|st, rb| {
        let (pk, pv) = dict(rb, "o_orderpriority");
        let pc: Vec<usize> = pv.iter().map(|v| PRIOS.iter().position(|p| p.as_bytes() == *v).expect("known priority")).collect();
        let ok = i64s(rb, "o_orderkey");
        for i in 0..rb.num_rows() {
            let (c, mut l) = chain_head(&t, ok[i]);
            let p = pc[pk[i] as usize];
            while l != END {
                let (_, (code, price)) = c.rows[l as usize];
                let a = &mut st[p * 7 + code as usize];
                a.0 += 1;
                a.1 += price as i128;
                l = c.next[l as usize];
            }
        }
    });
    let mut tot = [(0u64, 0i128); 35];
    for l in &locals {
        for g in 0..35 {
            tot[g].0 += l[g].0;
            tot[g].1 += l[g].1;
        }
    }
    struct R_([(u64, i128); 35]);
    impl Groups for R_ {
        fn for_each_group(&self, f: &mut dyn FnMut(&[KRef], u64, &[(i128, u64)])) {
            for (g, a) in self.0.iter().enumerate() {
                if a.0 > 0 {
                    f(&[KRef::S(PRIOS[g / 7].as_bytes()), KRef::S(MODES[g % 7].as_bytes())], a.0, &[(a.1, a.0)]);
                }
            }
        }
    }
    Ok(Box::new(R_(tot)))
}

pub fn run_fused(q: Query, inp: &Inputs, threads: usize) -> R<Box<dyn Groups>> {
    match q {
        Query::AggOrderkey => fused_agg_orderkey(inp, threads),
        Query::AggPartkey => fused_agg_partkey(inp, threads),
        Query::AggModeDate => fused_agg_modedate(inp, threads),
        Query::AggComment => fused_agg_comment(inp, threads),
        Query::Q3 => fused_q3(inp, threads),
        Query::Q12 => fused_q12(inp, threads),
        Query::J3 => fused_j3(inp, threads),
    }
}

#[allow(dead_code)]
fn unused(_: EngineError) {}
