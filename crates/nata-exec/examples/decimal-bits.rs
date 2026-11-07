//! Streaming bit-level oracle adapter: coefficient and scale on stdin.
use std::io::{self, BufRead, Write};
fn main() -> io::Result<()> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let line = line?;
        let (v, scale) = line.split_once(' ').expect("coefficient scale");
        writeln!(
            out,
            "{:016x}",
            nata_exec::num::decimal_to_f64(v.parse().expect("i128"), scale.parse().expect("scale"))
                .to_bits()
        )?;
    }
    Ok(())
}
