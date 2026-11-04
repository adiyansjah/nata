//! sqltool: prints the query lists and the DuckDB SQL of BOTH tables without building the engines (plain `rustc`,
//! no dependencies). The same command line as `shapes` / `gshapes` for `list` and `sql`. Used by `gcp-check.sh` in
//! its DuckDB-only mode (DS_NO_ENGINE=1), which validates the generators, the SQL and the comparison plumbing.
#![allow(dead_code)]
#[path = "qtable.rs"]
mod qtable;
#[path = "gtable.rs"]
mod gtable;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let h = qtable::queries();
    let g = gtable::queries();
    match args.get(1).map(String::as_str) {
        Some("list") => {
            for q in h.iter().filter(|q| args.get(2).is_none_or(|t| *t == q.tag)) {
                println!("{}\t{}\tok", q.name, q.tag);
            }
            for q in g.iter().filter(|q| args.get(2).is_none_or(|t| *t == q.tag)) {
                println!("{}\t{}\t{}", q.name, q.tag, if q.refuse { "refuse" } else { "ok" });
            }
        }
        Some("sql") => {
            let name = args.get(2).expect("query name");
            if let Some(q) = h.iter().find(|q| q.name == *name) {
                println!("{}", qtable::sql(&q.shape));
            } else {
                println!("{}", gtable::sql(g.iter().find(|q| q.name == *name).expect("unknown query")));
            }
        }
        _ => eprintln!("usage: sqltool list [tag] | sql <name>"),
    }
}
