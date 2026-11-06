//! TEST-ONLY helpers shared by the unit tests: the test set, a seeded generator, the mangler of the spike's fuzz corpus (d).

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
}

pub struct Row {
    pub q: String,
    pub expect: String,
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
        q: field("q"),
        expect: field("expect"),
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
