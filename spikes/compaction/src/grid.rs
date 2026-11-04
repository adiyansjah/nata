//! The benchmark grid: cells, checksum verification before timing, derived numbers (ratios, break-even) and the markdown report.

use std::fmt::Write as _;

use crate::arms::{Fixture, IDS, run_arm};
use crate::data::{Kind, Shape, make_column, make_selection, selected_count};
use crate::harness::{Arm, ArmStats, load1, measure, median, quantile, ratios};
use crate::reference::reference;

/// Expected result of arm `id` on `fx`, from the scalar reference (row by row through arrow's accessors).
pub fn expected(fx: &Fixture, id: &str) -> u64 {
    let sel_rows = fx.sel.rows();
    match id {
        "A" | "Bt" | "Bf" => reference(&fx.col, &sel_rows, 1),
        "A3" | "Ct" | "Cf" => reference(&fx.col, &sel_rows, 3),
        "D" => reference(&fx.col, &(0..fx.n).collect::<Vec<_>>(), 1),
        "Et" | "Ef" => sel_rows.len() as u64,
        _ => panic!("unknown arm {id}"),
    }
}

/// Run every arm once and compare with the reference. Err names the first arm that disagrees.
pub fn verify(fx: &Fixture) -> Result<(), String> {
    for id in IDS {
        let (got, want) = (run_arm(fx, id), expected(fx, id));
        if got != want {
            return Err(format!("arm {id}: checksum {got:#018x} != reference {want:#018x}"));
        }
    }
    Ok(())
}

pub struct Cell {
    pub kind: Kind,
    pub shape: Shape,
    pub sel_pct: f64,
    pub n: usize,
    pub nulls_pct: f64,
}

pub struct CellResult {
    pub cell: Cell,
    pub k: usize,
    pub arms: Vec<ArmStats>,
}

impl CellResult {
    pub fn arm(&self, id: &str) -> &ArmStats {
        self.arms.iter().find(|a| a.id == id).unwrap_or_else(|| panic!("no arm {id}"))
    }
    /// ns per SELECTED row, per round.
    pub fn per_row(&self, id: &str) -> Vec<f64> {
        self.arm(id).ns_per_call.iter().map(|x| x / self.k as f64).collect()
    }
    pub fn med_row(&self, id: &str) -> f64 {
        median(&self.per_row(id))
    }
}

pub fn build_fixture(c: &Cell, seed: u64) -> Fixture {
    let k = selected_count(c.n, c.sel_pct);
    let col = make_column(c.kind, c.n, c.nulls_pct, seed);
    let sel = make_selection(c.shape, c.n, k, seed ^ 0xA5A5);
    Fixture { col, sel, n: c.n, k }
}

/// Verify (an arm that disagrees with the reference aborts the cell: a fast wrong answer is not a data point), then time.
pub fn run_cell(c: Cell, rounds: usize, min_sample_ns: f64, seed: u64) -> Result<CellResult, String> {
    let fx = build_fixture(&c, seed);
    verify(&fx).map_err(|e| format!("{} {} sel {}% n {}: {e}", c.kind.key(), c.shape.key(), c.sel_pct, c.n))?;
    let arms: Vec<Arm<'_>> = IDS.iter().map(|&id| Arm { id: id.to_string(), run: Box::new({ let fx = &fx; move || run_arm(fx, id) }) }).collect();
    let arms = measure(&arms, rounds, min_sample_ns);
    let k = fx.k;
    Ok(CellResult { cell: c, k, arms })
}

// ------------------------------------------------------------------------------------------------ derived numbers

/// Paired median [q1, q3] of x/y over rounds.
#[derive(Clone, Copy)]
pub struct Ratio {
    pub med: f64,
    pub q1: f64,
    pub q3: f64,
}

fn ratio_of(r: &CellResult, x: &str, y: &str) -> Ratio {
    let v = ratios(&r.arm(x).ns_per_call, &r.arm(y).ns_per_call);
    Ratio { med: median(&v), q1: quantile(&v, 0.25), q3: quantile(&v, 0.75) }
}

pub struct Derived {
    /// The compaction method with the lower median for one function (B) and for three (C).
    pub b_id: &'static str,
    pub c_id: &'static str,
    pub b_over_a: Ratio,
    pub c_over_a3: Ratio,
    pub d_over_a: Ratio,
    /// Number of functions n at which compact-then-n-dense-passes (method of B) costs the same as n selection-aware passes:
    /// n* = 1 + (B - A) / (a - d), a = (A3 - A) / 2 and d = (C3 - B) / 2 the marginal cost of one more function
    /// (selection-aware / dense on the compacted column). None when a <= d (compaction never catches up).
    pub n_star: Option<f64>,
}

pub fn derive(r: &CellResult) -> Derived {
    let (b_id, bm) = if r.med_row("Bt") <= r.med_row("Bf") { ("Bt", "t") } else { ("Bf", "f") };
    let c_id = if r.med_row("Ct") <= r.med_row("Cf") { "Ct" } else { "Cf" };
    let cm = if bm == "t" { "Ct" } else { "Cf" };
    let (a1, a3, b, c3) = (r.med_row("A"), r.med_row("A3"), r.med_row(b_id), r.med_row(cm));
    let (a, d) = ((a3 - a1) / 2.0, (c3 - b) / 2.0);
    let n_star = if a > d { Some((1.0 + (b - a1) / (a - d)).max(1.0)) } else { None };
    Derived { b_id, c_id, b_over_a: ratio_of(r, b_id, "A"), c_over_a3: ratio_of(r, c_id, "A3"), d_over_a: ratio_of(r, "D", "A"), n_star }
}

pub enum BreakEven {
    /// Already at or below 1 at the lowest selectivity measured.
    AtOrBelow(f64),
    /// Interpolated (linearly in log10 of the selectivity) between two measured points.
    At(f64),
    /// Above 1 everywhere; the lowest ratio and where.
    Never(f64, f64),
}

/// First selectivity (ascending) at which the ratio `<= 1`. `pts` = (selectivity %, median ratio), ascending.
pub fn break_even(pts: &[(f64, f64)]) -> BreakEven {
    if pts[0].1 <= 1.0 {
        return BreakEven::AtOrBelow(pts[0].0);
    }
    for w in pts.windows(2) {
        let ((s0, r0), (s1, r1)) = (w[0], w[1]);
        if r1 <= 1.0 {
            let t = (r0 - 1.0) / (r0 - r1);
            return BreakEven::At(10f64.powf(s0.log10() + t * (s1.log10() - s0.log10())));
        }
    }
    let (s, r) = pts.iter().copied().fold((pts[0].0, pts[0].1), |m, p| if p.1 < m.1 { p } else { m });
    BreakEven::Never(r, s)
}

fn fmt_be(b: &BreakEven) -> String {
    match b {
        BreakEven::AtOrBelow(s) => format!("<= {}%", fmt_pct(*s)),
        BreakEven::At(s) => format!("~{}%", fmt_pct(*s)),
        BreakEven::Never(r, s) => format!("never (best {r:.2}x at {}%)", fmt_pct(*s)),
    }
}

pub fn fmt_pct(p: f64) -> String {
    if p < 1.0 { format!("{p:.2}") } else if p < 10.0 { format!("{p:.1}") } else { format!("{p:.0}") }
}

fn mq(v: &[f64]) -> String {
    format!("{:.2} [{:.2},{:.2}]", median(v), quantile(v, 0.25), quantile(v, 0.75))
}
fn rq(r: Ratio) -> String {
    format!("{:.2} [{:.2},{:.2}]", r.med, r.q1, r.q3)
}

// ------------------------------------------------------------------------------------------------ report

/// Markdown report for a set of results (all with the same NULL percentage and rounds).
pub fn report(results: &[CellResult], rounds: usize, nulls_pct: f64, load_start: f64, load_max: f64, load_end: f64) -> String {
    let mut o = String::new();
    let sanity = load_start >= 2.0 || load_max >= 2.0 || load_end >= 2.0;
    let tag = if sanity { " **(SANITY ONLY: load average >= 2)**" } else { "" };
    let _ = writeln!(o, "## Compaction vs carrying the selection, NULLs {nulls_pct}%{tag}\n");
    let _ = writeln!(o, "{rounds} interleaved rounds per cell (arm order rotated each round), one thread. Load average (1 min): start {load_start:.2}, highest between cells {load_max:.2}, end {load_end:.2}.");
    if sanity {
        let _ = writeln!(o, "\n**SANITY ONLY: the machine was not quiet. These numbers are not evidence.**");
    }
    let _ = writeln!(o, "\nAll times are ns per SELECTED row (arm time / k), median [lower quartile, upper quartile] over rounds. Ratios are paired per round: the ratio of the two arms' times from the same round, median [q1,q3]. D processes ALL n rows; its ns/row-selected is n*d/k, the cost of not selecting.\n");

    // Break-even summary
    let mut keys: Vec<(Kind, Shape, usize)> = Vec::new();
    for r in results {
        let key = (r.cell.kind, r.cell.shape, r.cell.n);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    let _ = writeln!(o, "### Break-even selectivity (median ratio <= 1, linear in log10(selectivity) between grid points; an estimate)\n");
    let _ = writeln!(o, "B/A: compact (best of take/filter) + 1 dense function against 1 selection-aware pass. C/A3: compact once + 3 dense functions against 3 selection-aware passes. D/A: dense pass over ALL rows against the selection-aware pass (below the break-even selection beats not selecting).\n");
    let _ = writeln!(o, "| type | selection | rows | B/A <= 1 at | C/A3 <= 1 at | D/A <= 1 at |");
    let _ = writeln!(o, "|---|---|---:|---|---|---|");
    for (kind, shape, n) in &keys {
        let mut cells: Vec<(f64, Derived)> = results.iter().filter(|r| (r.cell.kind, r.cell.shape, r.cell.n) == (*kind, *shape, *n)).map(|r| (r.cell.sel_pct, derive(r))).collect();
        cells.sort_by(|a, b| a.0.total_cmp(&b.0));
        let pts = |f: &dyn Fn(&Derived) -> f64| -> Vec<(f64, f64)> { cells.iter().map(|(s, d)| (*s, f(d))).collect() };
        let _ = writeln!(
            o,
            "| {} | {} | {n} | {} | {} | {} |",
            kind.name(),
            shape.name(),
            fmt_be(&break_even(&pts(&|d| d.b_over_a.med))),
            fmt_be(&break_even(&pts(&|d| d.c_over_a3.med))),
            fmt_be(&break_even(&pts(&|d| d.d_over_a.med)))
        );
    }

    // Detail tables
    let mut groups: Vec<(Kind, usize)> = Vec::new();
    for r in results {
        if !groups.contains(&(r.cell.kind, r.cell.n)) {
            groups.push((r.cell.kind, r.cell.n));
        }
    }
    for (kind, n) in groups {
        let _ = writeln!(o, "\n### {} , {n} rows, NULLs {nulls_pct}%\n", kind.name());
        let _ = writeln!(o, "| selection | sel % | k | A | A3 | Bt | Bf | Ct | Cf | D | Et | Ef | B*/A | C*/A3 | D/A | n* |");
        let _ = writeln!(o, "|---|---:|---:|---|---|---|---|---|---|---|---|---|---|---|---|---:|");
        for r in results.iter().filter(|r| r.cell.kind == kind && r.cell.n == n) {
            let d = derive(r);
            let cols: Vec<String> = IDS.iter().map(|id| mq(&r.per_row(id))).collect();
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {} | {} | {} | {} |",
                r.cell.shape.name(),
                fmt_pct(r.cell.sel_pct),
                r.k,
                cols.join(" | "),
                rq(d.b_over_a),
                rq(d.c_over_a3),
                rq(d.d_over_a),
                d.n_star.map_or("-".to_string(), |x| format!("{x:.1}"))
            );
        }
    }
    let _ = writeln!(o, "\nB* = the faster of Bt/Bf, C* the faster of Ct/Cf (by median). n* = number of functions at which compact-then-dense equals selection-aware (derived from A, A3, B*, C* by linear extrapolation of the marginal cost of one more function; '-' = compaction never catches up at any n).");
    o
}

/// CSV of every cell and arm: the full numbers behind the tables.
pub fn csv(results: &[CellResult]) -> String {
    let mut o = String::from("nulls_pct,type,shape,rows,sel_pct,k,arm,reps,median_ns_per_row,q1,q3,min\n");
    for r in results {
        for a in &r.arms {
            let v = r.per_row(&a.id);
            let min = v.iter().copied().fold(f64::INFINITY, f64::min);
            let _ = writeln!(o, "{},{},{},{},{},{},{},{},{:.4},{:.4},{:.4},{:.4}", r.cell.nulls_pct, r.cell.kind.key(), r.cell.shape.key(), r.cell.n, r.cell.sel_pct, r.k, a.id, a.reps, median(&v), quantile(&v, 0.25), quantile(&v, 0.75), min);
        }
    }
    o
}

/// Grid parameters.
pub struct GridOpts {
    pub kinds: Vec<Kind>,
    pub shapes: Vec<Shape>,
    pub sels: Vec<f64>,
    pub sizes: Vec<usize>,
    pub nulls_pct: f64,
    pub rounds: usize,
    pub min_sample_ns: f64,
}

impl GridOpts {
    pub fn default_grid(nulls_pct: f64, rounds: usize) -> Self {
        GridOpts {
            kinds: Kind::ALL.to_vec(),
            shapes: Shape::ALL.to_vec(),
            sels: vec![0.1, 1.0, 10.0, 50.0, 90.0, 100.0],
            sizes: vec![1024, 8192, 65536],
            nulls_pct,
            rounds,
            min_sample_ns: 100_000.0,
        }
    }
}

/// Run the whole grid; progress on stderr. Returns the results and the highest load seen between cells.
pub fn run_grid(o: &GridOpts) -> Result<(Vec<CellResult>, f64), String> {
    let total = o.kinds.len() * o.shapes.len() * o.sels.len() * o.sizes.len();
    let mut out = Vec::with_capacity(total);
    let mut load_max = load1();
    let mut seed = 1u64;
    for &kind in &o.kinds {
        for &n in &o.sizes {
            for &shape in &o.shapes {
                for &sel_pct in &o.sels {
                    seed += 1;
                    let c = Cell { kind, shape, sel_pct, n, nulls_pct: o.nulls_pct };
                    out.push(run_cell(c, o.rounds, o.min_sample_ns, seed)?);
                    load_max = load_max.max(load1());
                }
            }
            eprintln!("[{}/{}] {} n={} nulls={}% done, load1 {:.2}", out.len(), total, kind.key(), n, o.nulls_pct, load1());
        }
    }
    Ok((out, load_max))
}
