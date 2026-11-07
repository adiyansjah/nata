//! `nata sql` and `nata scan` as processes over objects in a store (card 0006 walking skeleton): the example query of the card with default settings, the same
//! rows as the identical local file, a glob, and what the process prints when the store refuses, fails or has no such object. The store is the fake endpoint
//! (loopback, plain http, a required access key); the credentials are canary strings whose appearance ANYWHERE in the output fails the test.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use nata_objfake::{FakeS3, Fault, Plan};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

const KEY_ID: &str = "AKIDCANARYNATAOBJ";
const SECRET: &str = "secret-canary-nata-object-store-0042";
const BUCKET: &str = "bucket-canary-nata";

fn parquet(rows: i64, group: usize, seed: i64) -> Vec<u8> {
    let id: ArrayRef = Arc::new(Int64Array::from_iter_values(
        (0..rows).map(|i| i + seed * 1000),
    ));
    let s: ArrayRef = Arc::new(StringArray::from_iter_values(
        (0..rows).map(|i| format!("v{}", i % 7)),
    ));
    let batch = RecordBatch::try_from_iter([("id", id), ("s", s)]).expect("batch");
    let mut out = Vec::new();
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(group))
        .build();
    let mut w = ArrowWriter::try_new(&mut out, batch.schema(), Some(props)).expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
    out
}

struct Rig {
    rt: tokio::runtime::Runtime,
    fake: FakeS3,
    dir: PathBuf,
}

impl Rig {
    fn new(name: &str) -> Rig {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        let fake = rt.block_on(FakeS3::start(BUCKET));
        fake.require_access_key(KEY_ID);
        let dir = std::env::temp_dir().join(format!("nata-objcli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        Rig { rt, fake, dir }
    }

    fn put(&self, key: &str, bytes: Vec<u8>) {
        std::fs::write(self.dir.join(key.replace('/', "_")), &bytes).expect("local copy");
        self.fake.put(key, bytes);
    }

    fn local(&self, key: &str) -> String {
        self.dir
            .join(key.replace('/', "_"))
            .to_string_lossy()
            .into_owned()
    }

    fn nata(&self, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_nata"));
        c.args(args)
            .env_remove("AI_AGENT")
            .env("AWS_ENDPOINT", &self.fake.addr)
            .env("AWS_ALLOW_HTTP", "true")
            .env("AWS_ACCESS_KEY_ID", KEY_ID)
            .env("AWS_SECRET_ACCESS_KEY", SECRET)
            .env("AWS_REGION", "us-east-1");
        c.output().expect("launch nata")
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = &self.rt;
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Nothing the process prints may carry a credential, the endpoint or the bucket name (the bucket appears in the URI the USER typed, so the test only
/// types it where the output may echo it, and looks for it separately).
fn no_secret(out: &Output, allow_bucket: bool) {
    let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
    let mut canaries = vec![KEY_ID, SECRET, "127.0.0.1", "Authorization", "x-amz"];
    if !allow_bucket {
        canaries.push(BUCKET);
    }
    for c in canaries {
        assert!(!all.contains(c), "the output carries {c:?}:\n{all}");
    }
}

#[test]
fn the_example_query_returns_the_rows_of_the_identical_local_file() {
    let rig = Rig::new("example");
    rig.put("t.parquet", parquet(5000, 700, 1));
    let remote = format!("SELECT id, s FROM 's3://{BUCKET}/t.parquet' WHERE id > 100 LIMIT 10");
    let local = format!(
        "SELECT id, s FROM '{}' WHERE id > 100 LIMIT 10",
        rig.local("t.parquet")
    );
    let (r, l) = (
        rig.nata(&["sql", "--csv", "-c", &remote]),
        rig.nata(&["sql", "--csv", "-c", &local]),
    );
    assert!(
        r.status.success() && l.status.success(),
        "{}",
        text(&r.stderr)
    );
    assert_eq!(text(&r.stdout), text(&l.stdout));
    assert_eq!(text(&r.stdout).lines().count(), 11);
    no_secret(&r, false);
    // the store saw requests signed with the environment's key and nothing else
    assert!(
        rig.fake
            .log()
            .iter()
            .all(|q| q.auth_key.as_deref() == Some(KEY_ID))
    );
}

#[test]
fn globs_scan_and_aggregates_equal_the_local_files() {
    let rig = Rig::new("glob");
    for i in 0..3 {
        rig.put(&format!("p/part-{i}.parquet"), parquet(3000, 500, i + 1));
    }
    let remote = format!("SELECT count(*), sum(id), min(s) FROM 's3://{BUCKET}/p/*.parquet'");
    let local = format!(
        "SELECT count(*), sum(id), min(s) FROM '{}'",
        rig.dir.join("p_part-*.parquet").to_string_lossy()
    );
    let (r, l) = (
        rig.nata(&["sql", "--csv", "-c", &remote]),
        rig.nata(&["sql", "--csv", "-c", &local]),
    );
    assert!(r.status.success(), "{}", text(&r.stderr));
    assert_eq!(text(&r.stdout), text(&l.stdout));
    // `nata scan` over explicit object paths
    let a = format!("s3://{BUCKET}/p/part-0.parquet");
    let b = format!("s3://{BUCKET}/p/part-1.parquet");
    let out = rig.nata(&[
        "scan",
        &a,
        &b,
        "--columns",
        "id",
        "--filter",
        "id >= 1000",
        "--csv",
        "--limit",
        "5",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).lines().count(), 6);
    no_secret(&out, true);
    // EXPLAIN reads no data but still binds from the footer
    let e = rig.nata(&[
        "sql",
        "-c",
        &format!("EXPLAIN SELECT id FROM 's3://{BUCKET}/p/part-0.parquet'"),
    ]);
    assert!(e.status.success(), "{}", text(&e.stderr));
}

#[test]
fn a_refused_failing_or_missing_object_is_an_error_that_names_no_secret() {
    let rig = Rig::new("errors");
    rig.put("t.parquet", parquet(2000, 500, 1));
    let q = |k: &str| format!("SELECT id FROM 's3://{BUCKET}/{k}'");
    // 404
    let o = rig.nata(&["sql", "--csv", "-c", &q("nope.parquet")]);
    assert_eq!(o.status.code(), Some(1), "a missing object is a data error");
    assert!(
        text(&o.stderr).contains("does not exist"),
        "{}",
        text(&o.stderr)
    );
    no_secret(&o, true);
    // 403 from the store: the key is refused
    rig.fake.require_access_key("AKIDSOMEONEELSE");
    let o = rig.nata(&["sql", "--csv", "-c", &q("t.parquet")]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("denied"), "{}", text(&o.stderr));
    no_secret(&o, true);
    // 500 forever with a long body: four attempts, then an error that quotes none of the body
    rig.fake.require_access_key(KEY_ID);
    rig.fake.set_plan(Plan::always(Fault::Status {
        code: 500,
        retry_after: None,
        body: 300,
        endless: false,
    }));
    let t = std::time::Instant::now();
    let o = rig.nata(&["sql", "--csv", "-c", &q("t.parquet")]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stderr).contains("server error") && text(&o.stderr).contains("4 attempts"),
        "{}",
        text(&o.stderr)
    );
    assert!(
        !text(&o.stderr).contains("xxxx"),
        "the response body must not be quoted"
    );
    no_secret(&o, true);
    assert!(t.elapsed().as_secs() < 30, "bounded: {:?}", t.elapsed());
    // the planted-leak check can fail: a canary in the text is found
    let planted = Output {
        status: o.status,
        stdout: SECRET.as_bytes().to_vec(),
        stderr: Vec::new(),
    };
    assert!(
        std::panic::catch_unwind(|| no_secret(&planted, true)).is_err(),
        "the leak check must be able to fail"
    );
}

#[test]
fn a_path_with_an_unsupported_scheme_is_not_supported_yet() {
    let rig = Rig::new("scheme");
    let o = rig.nata(&[
        "sql",
        "-c",
        "SELECT id FROM 'https://example.invalid/x.parquet'",
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        text(&o.stderr).contains("not supported yet"),
        "{}",
        text(&o.stderr)
    );
}
