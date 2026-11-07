//! `nata-objfake --dir <directory> [--bucket <name>] [--list-page <n>]`: serve the files under a directory as the objects of one bucket of a fake S3-style
//! endpoint on a loopback port, and print `endpoint=http://127.0.0.1:<port>` and `bucket=<name>`. A test fixture (no TLS, no real authentication): the
//! scripts that exercise `nata sql` against object storage without a cloud use it. It reads the directory once, at start.

use std::path::Path;

use nata_objfake::FakeS3;

fn walk(base: &Path, dir: &Path, fake: &FakeS3) -> std::io::Result<usize> {
    let mut n = 0;
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            n += walk(base, &path, fake)?;
        } else if let Ok(rel) = path.strip_prefix(base) {
            fake.put(&rel.to_string_lossy(), std::fs::read(&path)?);
            n += 1;
        }
    }
    Ok(n)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let Some(dir) = get("--dir") else {
        eprintln!("usage: nata-objfake --dir <directory> [--bucket <name>] [--list-page <n>]");
        std::process::exit(2);
    };
    let bucket = get("--bucket").unwrap_or_else(|| "bkt".into());
    let fake = FakeS3::start(&bucket).await;
    if let Some(n) = get("--list-page").and_then(|v| v.parse().ok()) {
        fake.set_list_page(n);
    }
    match walk(Path::new(&dir), Path::new(&dir), &fake) {
        Ok(n) => eprintln!("serving {n} objects"),
        Err(e) => {
            eprintln!("cannot read the directory: {e}");
            std::process::exit(1);
        }
    }
    println!("endpoint={}", fake.addr);
    println!("bucket={bucket}");
    // a script reads the two lines above, then kills this process
    std::future::pending::<()>().await;
}
