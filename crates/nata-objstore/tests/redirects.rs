//! Redirects are never followed (ADR 0011: "do not forward auth across redirect origins"). A hostile endpoint answers with 301, 302, 307 and 308 to a
//! second loopback listener; through the whole stack (store, credentials, HEAD, footer) the open ends at once in a message that names the status, the second
//! listener receives no connection (so no credential header is ever sent there), and no key, token or address is in the message. The wrapper-level cases
//! (`cases/fault.rs`, `redirect_case`) cover the same with the mutant sweep.

#[path = "../cases/mod.rs"]
mod cases;

use cases::fault::{REDIRECT_CANARIES, fake_s3};
use cases::testdata::lineitem_like;
use nata_objfake::{FakeS3, FakeSink, Fault, Plan};
use nata_objstore::store::GcsOptions;
use nata_objstore::{NodeOptions, ObjectHandle};
use nata_parquet::RemoteIo;
use object_store::aws::AmazonS3ConfigKey;
use object_store::gcp::GoogleConfigKey;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

fn open_text(h: &ObjectHandle, uri: &str) -> Result<usize, String> {
    let q = h.begin().map_err(|e| e.message)?;
    q.open(uri, &|| false)
        .map(|f| f.metadata.file_metadata().num_rows() as usize)
        .map_err(|e| e.message)
}

fn redirect(code: u16, sink: &FakeSink) -> Plan {
    let f = Fault::Redirect {
        code,
        location: Some(sink.addr.clone()),
    };
    Plan {
        sticky: Some(f.clone()),
        head: Some(f),
        ..Default::default()
    }
}

fn check(h: &ObjectHandle, uri: &str, fake: &FakeS3, sink: &FakeSink) {
    for code in [301u16, 302, 307, 308] {
        fake.set_plan(redirect(code, sink));
        let err = open_text(h, uri).expect_err("a redirect is an error");
        assert!(
            err.contains(&format!("HTTP {code}")) && err.contains("never followed"),
            "the message names the status: {err}"
        );
        assert!(err.len() < 200, "the message is short: {err}");
        for c in REDIRECT_CANARIES
            .iter()
            .copied()
            .chain([sink.addr.trim_start_matches("http://"), "withheld"])
        {
            assert!(!err.contains(c), "the message leaks a secret or an address");
        }
        assert_eq!(
            fake.log().len(),
            1,
            "one request, no retry, no follow ({code})"
        );
    }
    assert_eq!(sink.connections(), 0, "the redirect target was contacted");
    assert!(sink.requests().is_empty());
}

#[test]
fn s3_redirects_are_not_followed_and_the_session_token_stays_home() {
    let rt = rt();
    let fake = rt.block_on(FakeS3::start("bkt"));
    let sink = rt.block_on(FakeSink::start());
    fake.put("t.parquet", lineitem_like(3000, 1000, 1));
    let mut s3 = fake_s3(&fake);
    s3.config
        .retain(|(k, _)| *k != AmazonS3ConfigKey::SkipSignature);
    s3.config
        .push((AmazonS3ConfigKey::AccessKeyId, REDIRECT_CANARIES[0].into()));
    s3.config.push((
        AmazonS3ConfigKey::SecretAccessKey,
        REDIRECT_CANARIES[1].into(),
    ));
    s3.config
        .push((AmazonS3ConfigKey::Token, REDIRECT_CANARIES[2].into()));
    let mut o = NodeOptions::from_env();
    o.s3 = s3;
    o.cfg = nata_objstore::Config::fast_tests();
    let h = ObjectHandle::new(o).expect("node");
    assert_eq!(open_text(&h, "s3://bkt/t.parquet"), Ok(3000), "control");
    assert!(fake.log().iter().any(|r| r.session_token));
    check(&h, "s3://bkt/t.parquet", &fake, &sink);
}

#[test]
fn gcs_redirects_are_not_followed() {
    let rt = rt();
    let fake = rt.block_on(FakeS3::start("bkt"));
    let sink = rt.block_on(FakeSink::start());
    fake.put("t.parquet", lineitem_like(3000, 1000, 1));
    let mut o = NodeOptions::from_env();
    o.gcs = GcsOptions {
        config: vec![
            (GoogleConfigKey::BaseUrl, fake.addr.clone()),
            (GoogleConfigKey::SkipSignature, "true".into()),
        ],
        allow_http: true,
    };
    o.cfg = nata_objstore::Config::fast_tests();
    let h = ObjectHandle::new(o).expect("node");
    assert_eq!(open_text(&h, "gs://bkt/t.parquet"), Ok(3000), "control");
    check(&h, "gs://bkt/t.parquet", &fake, &sink);
}
