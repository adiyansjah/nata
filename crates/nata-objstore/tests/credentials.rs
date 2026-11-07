//! Rule 1 gate (a): the S3 credential chain, against the fake endpoint and a fake instance-metadata service. Static keys from the environment, a profile of
//! the shared credentials file, the instance metadata (IMDSv2) and their precedence; the fake endpoint answers 403 to a request signed by any other access
//! key, so a read that works proves which credentials signed it. Nothing secret reaches an error, a Debug string or a result.

#[path = "../cases/mod.rs"]
mod cases;

use cases::fault::fake_s3;
use cases::testdata::lineitem_like;
use nata_objfake::{FakeImds, FakeS3, ImdsCreds};
use nata_objstore::store::S3Options;
use nata_objstore::{NodeOptions, ObjectHandle};
use nata_parquet::RemoteIo;
use object_store::aws::AmazonS3ConfigKey;

const SECRET_ENV: &str = "env-secret-canary-0001";
const SECRET_PROFILE: &str = "profile-secret-canary-0002";
const SECRET_IMDS: &str = "imds-secret-canary-0003";

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

fn node(s3: S3Options) -> ObjectHandle {
    let mut o = NodeOptions::from_env();
    o.s3 = s3;
    o.cfg = nata_objstore::Config::fast_tests();
    ObjectHandle::new(o).expect("node")
}

/// Open the object through the whole stack (store, credentials, HEAD, footer): the error text, if any.
fn open_text(h: &ObjectHandle) -> Result<usize, String> {
    let q = h.begin().map_err(|e| e.message)?;
    q.open("s3://bkt/t.parquet", &|| false)
        .map(|f| f.metadata.file_metadata().num_rows() as usize)
        .map_err(|e| e.message)
}

fn with_keys(fake: &FakeS3, key: &str, secret: &str) -> S3Options {
    let mut o = fake_s3(fake);
    o.config
        .retain(|(k, _)| *k != AmazonS3ConfigKey::SkipSignature);
    o.config.push((AmazonS3ConfigKey::AccessKeyId, key.into()));
    o.config
        .push((AmazonS3ConfigKey::SecretAccessKey, secret.into()));
    o
}

fn setup() -> (tokio::runtime::Runtime, FakeS3) {
    let rt = rt();
    let fake = rt.block_on(FakeS3::start("bkt"));
    fake.put("t.parquet", lineitem_like(3000, 1000, 1));
    (rt, fake)
}

fn keys_seen(fake: &FakeS3) -> Vec<Option<String>> {
    let mut v: Vec<Option<String>> = fake.log().into_iter().map(|r| r.auth_key).collect();
    v.dedup();
    v
}

#[test]
fn environment_keys_sign_every_request() {
    let (_rt, fake) = setup();
    fake.require_access_key("AKIDENV");
    let h = node(with_keys(&fake, "AKIDENV", SECRET_ENV));
    assert_eq!(open_text(&h), Ok(3000));
    assert_eq!(
        keys_seen(&fake),
        vec![Some("AKIDENV".to_string())],
        "HEAD and footer GET were signed by the environment's key"
    );
}

#[test]
fn another_key_is_refused_and_the_error_names_no_secret() {
    let (_rt, fake) = setup();
    fake.require_access_key("AKIDRIGHT");
    let h = node(with_keys(&fake, "AKIDWRONG", SECRET_ENV));
    let e = open_text(&h).expect_err("403");
    assert!(e.contains("denied"), "{e}");
    for canary in [
        "AKIDWRONG",
        SECRET_ENV,
        "AKIDRIGHT",
        "127.0.0.1",
        "bkt",
        "t.parquet",
    ] {
        assert!(!e.contains(canary), "the error leaks {canary:?}: {e}");
    }
    let debug = format!("{:?} {:?}", h, with_keys(&fake, "AKIDWRONG", SECRET_ENV));
    assert!(
        !debug.contains(SECRET_ENV),
        "Debug must not show a secret: {debug}"
    );
}

#[test]
fn a_profile_of_the_shared_credentials_file_supplies_static_keys() {
    let (_rt, fake) = setup();
    fake.require_access_key("AKIDPROFILE");
    let dir = std::env::temp_dir().join(format!("nata-objstore-profile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("credentials");
    std::fs::write(&file, format!("[default]\naws_access_key_id = AKIDDEFAULT\naws_secret_access_key = other\n\n[team]\naws_access_key_id = AKIDPROFILE\naws_secret_access_key = {SECRET_PROFILE}\n")).expect("write");
    let mut s3 = fake_s3(&fake);
    s3.config
        .retain(|(k, _)| *k != AmazonS3ConfigKey::SkipSignature);
    s3.profile = Some("team".into());
    s3.credentials_file = Some(file.clone());
    let h = node(s3.clone());
    assert_eq!(open_text(&h), Ok(3000));
    assert_eq!(keys_seen(&fake), vec![Some("AKIDPROFILE".to_string())]);

    // explicit environment keys win over a profile
    fake.require_access_key("AKIDENV");
    let mut both = s3.clone();
    both.config
        .push((AmazonS3ConfigKey::AccessKeyId, "AKIDENV".into()));
    both.config
        .push((AmazonS3ConfigKey::SecretAccessKey, SECRET_ENV.into()));
    let h = node(both);
    assert_eq!(
        open_text(&h),
        Ok(3000),
        "keys from the environment take precedence over the profile"
    );

    // a profile that is not in the file is a clear error, and the message holds no key
    let mut missing = s3;
    missing.profile = Some("nope".into());
    let h = node(missing);
    let e = open_text(&h).expect_err("no such profile");
    for canary in ["AKIDPROFILE", "AKIDDEFAULT", SECRET_PROFILE, "other"] {
        assert!(!e.contains(canary), "{e}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn instance_metadata_supplies_credentials_when_nothing_else_does() {
    let (rt, fake) = setup();
    fake.require_access_key("AKIDIMDS");
    let imds = rt.block_on(FakeImds::start(ImdsCreds {
        key_id: "AKIDIMDS".into(),
        secret: SECRET_IMDS.into(),
        token: Some("imds-session-token-canary".into()),
    }));
    let mut s3 = fake_s3(&fake);
    s3.config
        .retain(|(k, _)| *k != AmazonS3ConfigKey::SkipSignature);
    s3.config
        .push((AmazonS3ConfigKey::MetadataEndpoint, imds.addr.clone()));
    let h = node(s3);
    assert_eq!(open_text(&h), Ok(3000));
    assert_eq!(
        keys_seen(&fake),
        vec![Some("AKIDIMDS".to_string())],
        "the S3 requests were signed with the key the metadata service gave"
    );
    assert!(
        fake.log().iter().all(|r| r.session_token),
        "the session token of the metadata credentials accompanies every request"
    );
    let asked = imds.requests();
    assert_eq!(
        asked.first().map(String::as_str),
        Some("PUT /latest/api/token"),
        "IMDSv2: the token first: {asked:?}"
    );
    assert!(
        asked
            .iter()
            .any(|r| r == "GET /latest/meta-data/iam/security-credentials/fake-role"),
        "{asked:?}"
    );
    // the credentials are cached for the node's life: a second query asks the metadata service no more
    let before = imds.requests().len();
    let q = h.begin().expect("query");
    q.open("s3://bkt/t.parquet", &|| false).expect("open");
    assert_eq!(
        imds.requests().len(),
        before,
        "credentials are reused until they near expiry"
    );
}

#[test]
fn a_dead_metadata_service_is_a_bounded_clear_error() {
    let (rt, fake) = setup();
    let imds = rt.block_on(FakeImds::start(ImdsCreds {
        key_id: "AKIDIMDS".into(),
        secret: SECRET_IMDS.into(),
        token: None,
    }));
    let addr = imds.addr.clone();
    drop(imds); // nothing listens there any more
    let mut s3 = fake_s3(&fake);
    s3.config
        .retain(|(k, _)| *k != AmazonS3ConfigKey::SkipSignature);
    s3.config.push((AmazonS3ConfigKey::MetadataEndpoint, addr));
    let h = node(s3);
    let t = std::time::Instant::now();
    let e = open_text(&h).expect_err("no credentials");
    assert!(t.elapsed().as_secs() < 60, "bounded: {:?}", t.elapsed());
    assert!(!e.contains("127.0.0.1"), "{e}");
}
