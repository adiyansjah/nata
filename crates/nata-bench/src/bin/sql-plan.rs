//! In-process parse + bind measurement, including the filesystem schema lookup.
use std::{hint::black_box, time::Instant};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let sql = std::fs::read_to_string(&args[1])?;
    // A warm-up, then five paired-session samples (the driver alternates engines).
    let _ = nata_sql::plan_sql(&sql, &nata_sql::FsSource);
    for _ in 0..20 {
        let start = Instant::now();
        let plan = nata_sql::plan_sql(black_box(&sql), &nata_sql::FsSource);
        let seconds = start.elapsed().as_secs_f64();
        let _ = black_box(plan);
        println!("{seconds:.9}");
    }
    Ok(())
}
