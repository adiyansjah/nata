//! The smallest program that uses the parsers, to size a build with and without arm A: `parse_cli [--a] SQL` prints the AST (or the error).
use spike_sql_parser::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (use_a, sql) = match args.get(1).map(String::as_str) {
        Some("--a") => (true, args.get(2).cloned().unwrap_or_default()),
        _ => (false, args.get(1).cloned().unwrap_or_default()),
    };
    let r = if use_a {
        #[cfg(feature = "arm-a")]
        {
            sqlp::parse(&sql)
        }
        #[cfg(not(feature = "arm-a"))]
        {
            hand::parse(&sql)
        }
    } else {
        hand::parse(&sql)
    };
    match r {
        Ok(q) => println!("{}", q.sexp()),
        Err(e) => println!("{e}"),
    }
}
