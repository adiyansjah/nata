//! TEST-ONLY helpers shared by the unit tests: the test set, a seeded generator, the mangler of the spike's fuzz corpus (d), a generator of expressions.

/// splitmix64: small, seeded, the same on every machine.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % (n.max(1) as u64)).unwrap_or(0)
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    pub fn chance(&mut self, num: usize, den: usize) -> bool {
        self.below(den) < num
    }
}

pub struct Row {
    pub id: String,
    pub q: String,
    pub expect: String,
    pub tag: String,
}

/// The test set (`corpus/testset.jsonl`, written by `corpus/testset.py`): one JSON object per line with only these escapes.
pub fn testset() -> Vec<Row> {
    let text =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus/testset.jsonl"))
            .expect("testset.jsonl");
    text.lines().map(row).collect()
}

fn row(line: &str) -> Row {
    let field = |name: &str| -> String {
        let key = format!("\"{name}\": \"");
        let s = line.find(&key).expect("field") + key.len();
        let mut out = String::new();
        let mut it = line[s..].chars();
        while let Some(c) = it.next() {
            match c {
                '"' => break,
                '\\' => match it.next() {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('u') => {
                        let hex: String = it.by_ref().take(4).collect();
                        out.push(
                            char::from_u32(u32::from_str_radix(&hex, 16).expect("hex"))
                                .expect("char"),
                        );
                    }
                    other => panic!("escape {other:?}"),
                },
                c => out.push(c),
            }
        }
        out
    };
    Row {
        id: field("id"),
        q: field("q"),
        expect: field("expect"),
        tag: field("tag"),
    }
}

pub fn accepted_queries() -> Vec<String> {
    testset()
        .into_iter()
        .filter(|r| r.expect == "accept")
        .map(|r| r.q)
        .collect()
}

const ALPHABET: &str = "'\"(),;*+-/%=<>!.:|$?[]{}@#&^~ _\n\t0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const EXTRA: [char; 4] = ['é', '日', '😀', ' '];

/// Mangled query `i` of `seed` (corpus (d) of the bake-off): one edit (delete, swap two adjacent characters, insert one) of an accepted test-set query.
/// Query `i` depends only on `(seed, i)`, so a failure replays from the two numbers.
pub fn mangled(base: &[String], seed: u64, i: u64) -> String {
    let mut rng = Rng(seed.wrapping_mul(7_919).wrapping_add(i));
    let mut c: Vec<char> = rng.pick(base).chars().collect();
    let mut op = rng.below(3);
    if op == 1 {
        // swap needs two different adjacent characters
        let cands: Vec<usize> = (0..c.len().saturating_sub(1))
            .filter(|&k| c[k] != c[k + 1])
            .collect();
        if cands.is_empty() {
            op = 0;
        } else {
            let k = *rng.pick(&cands);
            c.swap(k, k + 1);
            return c.into_iter().collect();
        }
    }
    if op == 0 && !c.is_empty() {
        let k = rng.below(c.len());
        c.remove(k);
    } else {
        let k = rng.below(c.len() + 1);
        let alphabet: Vec<char> = ALPHABET.chars().chain(EXTRA).collect();
        let ch = *rng.pick(&alphabet);
        c.insert(k, ch);
    }
    c.into_iter().collect()
}

/// A random expression over columns `a b c d f s dt` (the generated corpus (c) of the bake-off, in miniature): rendered once with every operator
/// parenthesised and once with the fewest parentheses that SQL precedence allows.
#[derive(Clone, Debug)]
pub enum G {
    Col(&'static str),
    Num(&'static str),
    Str(&'static str),
    Date,
    Null,
    Bool(bool),
    Neg(Box<G>),
    Not(Box<G>),
    Bin(&'static str, u8, Box<G>, Box<G>),
    Between(Box<G>, Box<G>, Box<G>, bool),
    In(Box<G>, Vec<G>, bool),
    IsNull(Box<G>, bool),
    Cast(Box<G>, &'static str),
    Case(Option<Box<G>>, Vec<(G, G)>, Option<Box<G>>),
}

const COLS: [&str; 7] = ["a", "b", "c", "d", "f", "s", "dt"];
const NUMS: [&str; 10] = [
    "0",
    "1",
    "42",
    "2147483648",
    "1.5",
    "0.10",
    "0.05",
    ".5",
    "1e2",
    "123456789012345678901234567890.12",
];
const BIN: [(&str, u8); 13] = [
    ("OR", 1),
    ("AND", 2),
    ("=", 5),
    ("<>", 5),
    ("<", 5),
    (">=", 5),
    ("+", 7),
    ("-", 7),
    ("*", 8),
    ("/", 8),
    ("%", 8),
    ("//", 8),
    ("<=", 5),
];
const TYPES: [&str; 7] = [
    "BOOLEAN",
    "INTEGER",
    "BIGINT",
    "DOUBLE",
    "DECIMAL(10,2)",
    "VARCHAR",
    "DATE",
];

pub fn gen_expr(rng: &mut Rng, depth: usize) -> G {
    if depth == 0 || rng.chance(1, 4) {
        return match rng.below(8) {
            0..=2 => G::Col(rng.pick(&COLS)),
            3..=4 => G::Num(rng.pick(&NUMS)),
            5 => G::Str(rng.pick(&["x", "it''s", "", "日本"])),
            6 => rng
                .pick(&[G::Date, G::Null, G::Bool(true), G::Bool(false)])
                .clone(),
            _ => G::Col("a"),
        };
    }
    let d = depth - 1;
    match rng.below(10) {
        0 => G::Neg(Box::new(gen_expr(rng, d))),
        1 => G::Not(Box::new(gen_expr(rng, d))),
        2 => G::Between(
            Box::new(gen_expr(rng, d)),
            Box::new(gen_expr(rng, d)),
            Box::new(gen_expr(rng, d)),
            rng.chance(1, 3),
        ),
        3 => {
            let n = 1 + rng.below(3);
            G::In(
                Box::new(gen_expr(rng, d)),
                (0..n).map(|_| gen_expr(rng, 0)).collect(),
                rng.chance(1, 3),
            )
        }
        4 => G::IsNull(Box::new(gen_expr(rng, d)), rng.chance(1, 2)),
        5 => G::Cast(Box::new(gen_expr(rng, d)), rng.pick(&TYPES)),
        6 => {
            let n = 1 + rng.below(2);
            G::Case(
                rng.chance(1, 3).then(|| Box::new(gen_expr(rng, d))),
                (0..n)
                    .map(|_| (gen_expr(rng, d), gen_expr(rng, d)))
                    .collect(),
                rng.chance(1, 2).then(|| Box::new(gen_expr(rng, d))),
            )
        }
        _ => {
            let (op, p) = *rng.pick(&BIN);
            G::Bin(
                op,
                p,
                Box::new(gen_expr(rng, d)),
                Box::new(gen_expr(rng, d)),
            )
        }
    }
}

impl G {
    /// The precedence level of the node when it is the operand of another operator (higher binds tighter).
    fn level(&self) -> u8 {
        match self {
            G::Bin(_, p, ..) => *p,
            G::Not(_) => 3,
            G::IsNull(..) => 4,
            G::Between(..) | G::In(..) => 6,
            G::Neg(_) => 9,
            _ => 10,
        }
    }

    /// `minimal`: the fewest parentheses; otherwise every compound operand is parenthesised.
    pub fn render(&self, minimal: bool) -> String {
        match self {
            G::Col(c) => (*c).to_string(),
            G::Num(n) => (*n).to_string(),
            G::Str(s) => format!("'{s}'"),
            G::Date => "DATE '1994-01-01'".into(),
            G::Null => "NULL".into(),
            G::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
            G::Neg(e) => format!("-{}", e.operand(minimal, 9, true)),
            G::Not(e) => format!("NOT {}", e.operand(minimal, 3, true)),
            G::Bin(op, p, l, r) => {
                // comparisons do not chain, and IS/BETWEEN/IN/comparison share no associativity: always parenthesise a child at the same level 4..6
                let lp = if *p == 5 { 6 } else { *p };
                format!(
                    "{} {} {}",
                    l.operand(minimal, lp, false),
                    op,
                    r.operand(minimal, if *p == 5 { 6 } else { *p + 1 }, false)
                )
            }
            G::Between(x, lo, hi, n) => format!(
                "{} {}BETWEEN {} AND {}",
                x.operand(minimal, 7, false),
                if *n { "NOT " } else { "" },
                lo.operand(minimal, 7, false),
                hi.operand(minimal, 7, false)
            ),
            G::In(x, l, n) => format!(
                "{} {}IN ({})",
                x.operand(minimal, 7, false),
                if *n { "NOT " } else { "" },
                l.iter()
                    .map(|e| e.render(minimal))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            G::IsNull(x, n) => format!(
                "{} IS {}NULL",
                x.operand(minimal, 5, false),
                if *n { "NOT " } else { "" }
            ),
            G::Cast(x, t) => format!("CAST({} AS {t})", x.render(minimal)),
            G::Case(op, whens, els) => {
                let mut s = String::from("CASE");
                if let Some(o) = op {
                    s.push(' ');
                    s.push_str(&o.render(minimal));
                }
                for (c, v) in whens {
                    s.push_str(&format!(
                        " WHEN {} THEN {}",
                        c.render(minimal),
                        v.render(minimal)
                    ));
                }
                if let Some(e) = els {
                    s.push_str(&format!(" ELSE {}", e.render(minimal)));
                }
                s.push_str(" END");
                s
            }
        }
    }

    fn operand(&self, minimal: bool, need: u8, prefix: bool) -> String {
        let s = self.render(minimal);
        let compound = !matches!(
            self,
            G::Col(_)
                | G::Num(_)
                | G::Str(_)
                | G::Date
                | G::Null
                | G::Bool(_)
                | G::Cast(..)
                | G::Case(..)
        );
        let lvl = self.level();
        // a prefix operator's operand: a number after `-` is fine, a sum is not; `NOT` takes anything at its level or tighter
        let needs = if minimal {
            compound && (lvl < need || (prefix && matches!(self, G::Neg(_)) && false))
        } else {
            compound
        };
        // `-` in front of a number would make `--1` (a comment): parenthesise a negative operand of a negation
        let dash = prefix && s.starts_with('-') && need == 9;
        if needs || dash { format!("({s})") } else { s }
    }
}

/// A complete query around an expression.
pub fn query_of(e: &G, minimal: bool, where_: bool) -> String {
    let s = e.render(minimal);
    if where_ {
        format!("SELECT a FROM 'f.parquet' WHERE {s}")
    } else {
        format!("SELECT {s} AS r FROM 'f.parquet'")
    }
}
