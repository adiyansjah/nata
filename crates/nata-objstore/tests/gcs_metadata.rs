//! Exercise the real VM credential chain in isolated processes: changing GCE_*
//! variables in a multithreaded test would race other credential tests.
use std::process::Command;
use std::time::Duration;

use nata_objfake::{FakeImds, FakeS3, ImdsFault};
use nata_objstore::remote::{BoundedConnector, ClientSettings};
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::{ClientOptions, ObjectStore, ObjectStoreExt, RetryConfig};

fn connector() -> BoundedConnector {
    BoundedConnector {
        cap: 4096,
        meta_cap: 8 << 20,
        bound_success: true,
        client: ClientSettings {
            connect_timeout: Duration::from_secs(1),
            timeout: Some(Duration::from_secs(3)),
            pool_max_idle_per_host: 2,
            follow_redirects: false,
        },
    }
}

#[test]
fn vm_credentials_use_http_without_allowing_http_objects() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    if let Ok(mode) = std::env::var("NATA_GCS_METADATA_CHILD") {
        rt.block_on(async {
            let store = GoogleCloudStorageBuilder::new()
                .with_bucket_name("bkt")
                .with_base_url(&std::env::var("NATA_GCS_OBJECT_ENDPOINT").expect("endpoint"))
                .with_client_options(ClientOptions::new().with_allow_http(mode == "object-ok"))
                .with_retry(RetryConfig {
                    max_retries: 0,
                    ..Default::default()
                })
                .with_http_connector(connector())
                .build()
                .expect("store");
            let creds = store.credentials();
            let first = creds.get_credential().await;
            if mode == "ok" || mode == "object-ok" {
                assert_eq!(first.expect("VM metadata token").bearer, "fake-gce-token");
                // An immediately expiring token must refresh through the same bounded client.
                assert_eq!(
                    creds.get_credential().await.expect("refresh").bearer,
                    "fake-gce-token"
                );
                // Even after a metadata client allowed HTTP, the object client must reject it.
                if mode == "ok" {
                    assert!(store.head(&"t.parquet".into()).await.is_err());
                } else {
                    let path = "t.parquet".into();
                    assert_eq!(store.head(&path).await.expect("HEAD").size, 32);
                    let result = store
                        .get_opts(
                            &path,
                            object_store::GetOptions {
                                range: Some(object_store::GetRange::Bounded(3..9)),
                                ..Default::default()
                            },
                        )
                        .await
                        .expect("range");
                    assert_eq!(result.bytes().await.expect("body").as_ref(), &[1; 6]);
                }
            } else {
                assert!(first.is_err(), "hostile metadata was accepted");
            }
        });
        return;
    }
    let fake = rt.block_on(FakeImds::start_gcs("fake-gce-token", 0));
    let objects = rt.block_on(FakeS3::start("bkt"));
    objects.put("t.parquet", vec![1; 32]);
    let home = std::env::temp_dir().join(format!("nata-gcs-metadata-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("empty home without ADC");
    for (mode, fault) in [
        ("ok", None),
        ("object-ok", None),
        ("endless", Some(ImdsFault::EndlessCredentials)),
        (
            "oversize",
            Some(ImdsFault::Credentials(vec![b' '; (8 << 20) + 1])),
        ),
        (
            "invalid",
            Some(ImdsFault::Credentials(b"not JSON".to_vec())),
        ),
    ] {
        fake.set_fault(fault);
        let before = fake.requests().len();
        let out = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "vm_credentials_use_http_without_allowing_http_objects",
                "--nocapture",
            ])
            .env("NATA_GCS_METADATA_CHILD", mode)
            .env("GCE_METADATA_HOST", fake.addr.trim_start_matches("http://"))
            .env("GCE_METADATA_IP", fake.addr.trim_start_matches("http://"))
            .env("NATA_GCS_OBJECT_ENDPOINT", &objects.addr)
            .env("HOME", &home)
            .env("NO_PROXY", "*")
            .env_remove("GOOGLE_APPLICATION_CREDENTIALS")
            .output()
            .expect("child");
        assert!(
            out.status.success(),
            "{mode}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let requests = fake.requests();
        assert!(
            requests.len() >= before + 2,
            "{mode}: metadata HTTP was never reached"
        );
        assert!(requests[before..].iter().all(|r| {
            r.starts_with("GET /computeMetadata/v1/instance/service-accounts/default/token?")
        }));
        if mode == "object-ok" {
            let log = objects.log();
            assert_eq!(log.len(), 2);
            assert!(log.iter().all(|r| r.bearer));
            objects.set_plan(Default::default()); // resets the request log
        } else {
            assert!(
                objects.log().is_empty(),
                "TLS-only object client sent plaintext"
            );
        }
    }
    std::fs::remove_dir_all(home).expect("cleanup");
}

#[test]
fn invalid_http_permission_is_rejected_without_echoing_its_value() {
    use object_store::client::HttpConnector;
    let options = ClientOptions::new().with_config(
        object_store::ClientConfigKey::AllowHttp,
        "invalid-permission-canary",
    );
    let result = connector().connect(&options);
    assert!(result.is_err());
    let text = result.expect_err("invalid policy").to_string();
    assert!(text.contains("invalid HTTP transport policy"));
    assert!(!text.contains("invalid-permission-canary"));
}
