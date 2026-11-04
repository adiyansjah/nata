//! Building the object store under test, and keeping the real one's coordinates out of every file and log.
//!
//! Two modes, both through the same client stack (`object_store`'s S3-API client, which is also what GCS's S3
//! interoperability endpoint speaks):
//!   * SIMULATED: `http://127.0.0.1:PORT`, anonymous, started by `gcp-check.sh` (see `sim.rs`);
//!   * REAL: endpoint, bucket, prefix and (optional) HMAC credentials come ONLY from environment variables. Nothing
//!     here prints them, and every message that may carry them goes through [`Redactor`].

use std::sync::Arc;
use std::time::Duration;

use object_store::aws::AmazonS3Builder;
use object_store::local::LocalFileSystem;
use object_store::path::Path;
use object_store::{BackoffConfig, ClientOptions, ObjectStore, RetryConfig};

use crate::util::env_nonempty;

pub const ENV_ENDPOINT: &str = "NATA_OBJSTORE_ENDPOINT";
pub const ENV_BUCKET: &str = "NATA_OBJSTORE_BUCKET";
pub const ENV_PREFIX: &str = "NATA_OBJSTORE_PREFIX";
pub const ENV_REGION: &str = "NATA_OBJSTORE_REGION";
pub const ENV_KEY_ID: &str = "NATA_OBJSTORE_ACCESS_KEY_ID";
pub const ENV_SECRET: &str = "NATA_OBJSTORE_SECRET_ACCESS_KEY";
pub const ENV_LABEL: &str = "NATA_OBJSTORE_LABEL";
pub const ENV_HTTP1: &str = "NATA_OBJSTORE_HTTP1";
pub const ENV_ALLOW_HTTP: &str = "NATA_OBJSTORE_ALLOW_HTTP";

#[derive(Clone)]
pub struct StoreSpec {
    pub endpoint: String,
    pub bucket: String,
    pub prefix: String,
    pub region: String,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub allow_http: bool,
    pub http1_only: bool,
    /// A free-text, NON-SECRET description the operator supplies ("GCS S3-interop, same region as the VM").
    pub label: String,
    pub simulated: bool,
}

// Debug must never show the coordinates.
impl std::fmt::Debug for StoreSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StoreSpec({}, <endpoint, bucket, prefix and keys hidden>)", if self.simulated { "SIMULATED" } else { "REAL" })
    }
}

pub enum RealConfig {
    /// Nothing configured: the caller must say `REAL STORE NOT MEASURED`.
    NotConfigured,
    /// Some but not all of endpoint/bucket/prefix: refuse loudly.
    Incomplete(Vec<&'static str>),
    Configured(StoreSpec),
}

impl StoreSpec {
    pub fn sim(addr: &str, bucket: &str) -> StoreSpec {
        StoreSpec {
            endpoint: format!("http://{addr}"),
            bucket: bucket.to_string(),
            prefix: String::new(),
            region: "us-east-1".into(),
            access_key: None,
            secret_key: None,
            allow_http: true,
            http1_only: false,
            label: "SIMULATED local server (loopback)".into(),
            simulated: true,
        }
    }

    pub fn real_from_env() -> RealConfig {
        let ep = env_nonempty(ENV_ENDPOINT);
        let bucket = env_nonempty(ENV_BUCKET);
        let prefix = env_nonempty(ENV_PREFIX);
        if ep.is_none() && bucket.is_none() && prefix.is_none() {
            return RealConfig::NotConfigured;
        }
        let mut missing = Vec::new();
        if ep.is_none() {
            missing.push(ENV_ENDPOINT);
        }
        if bucket.is_none() {
            missing.push(ENV_BUCKET);
        }
        if prefix.is_none() {
            missing.push(ENV_PREFIX);
        }
        if !missing.is_empty() {
            return RealConfig::Incomplete(missing);
        }
        let (ep, bucket, prefix) = (ep.unwrap_or_default(), bucket.unwrap_or_default(), prefix.unwrap_or_default());
        let key = env_nonempty(ENV_KEY_ID);
        let secret = env_nonempty(ENV_SECRET);
        let plain_http = ep.starts_with("http://");
        let allow_http = env_nonempty(ENV_ALLOW_HTTP).as_deref() == Some("1");
        let (access_key, secret_key) = match (key, secret) {
            (Some(k), Some(s)) => (Some(k), Some(s)),
            _ => (None, None),
        };
        RealConfig::Configured(StoreSpec {
            endpoint: ep,
            bucket,
            prefix: prefix.trim_matches('/').to_string(),
            region: env_nonempty(ENV_REGION).unwrap_or_else(|| "auto".into()),
            // credentials over plain http only when the operator said so explicitly
            allow_http: plain_http && (allow_http || access_key.is_none()),
            http1_only: env_nonempty(ENV_HTTP1).as_deref() == Some("1"),
            access_key,
            secret_key,
            label: env_nonempty(ENV_LABEL).unwrap_or_else(|| "REAL store (no label supplied)".into()),
            simulated: false,
        })
    }

    pub fn has_credentials(&self) -> bool {
        self.access_key.is_some()
    }

    /// Path of `<layout>/<file>` inside the bucket (the prefix is prepended).
    pub fn path(&self, rel: &str) -> Path {
        if self.prefix.is_empty() {
            Path::from(rel)
        } else {
            Path::from(format!("{}/{}", self.prefix, rel))
        }
    }

    /// `builtin_retries`: `None` disables the crate's own retries (the engine's retry layer is under test);
    /// `Some(n)` enables `object_store`'s own retry with that many retries (the reference arm of the fault tests).
    pub fn build(&self, builtin_retries: Option<usize>) -> Result<Arc<dyn ObjectStore>, String> {
        let mut b = AmazonS3Builder::new()
            .with_endpoint(&self.endpoint)
            .with_bucket_name(&self.bucket)
            .with_region(&self.region)
            .with_virtual_hosted_style_request(false)
            .with_allow_http(self.allow_http);
        match (&self.access_key, &self.secret_key) {
            (Some(k), Some(s)) => b = b.with_access_key_id(k).with_secret_access_key(s),
            _ => b = b.with_skip_signature(true),
        }
        let mut co = ClientOptions::new()
            .with_allow_http(self.allow_http)
            .with_pool_max_idle_per_host(64)
            .with_connect_timeout(Duration::from_secs(5))
            .with_timeout(Duration::from_secs(120));
        if self.http1_only {
            co = co.with_http1_only();
        }
        b = b.with_client_options(co);
        let retry = match builtin_retries {
            None => RetryConfig {
                backoff: BackoffConfig::default(),
                max_retries: 0,
                retry_timeout: Duration::from_secs(30),
            },
            Some(n) => RetryConfig {
                backoff: BackoffConfig {
                    init_backoff: Duration::from_millis(40),
                    max_backoff: Duration::from_secs(2),
                    base: 2.0,
                },
                max_retries: n,
                retry_timeout: Duration::from_secs(30),
            },
        };
        b = b.with_retry(retry);
        let s = b.build().map_err(|e| self.redactor().apply(&e.to_string()))?;
        Ok(Arc::new(s))
    }

    pub fn redactor(&self) -> Redactor {
        let mut v = vec![
            self.endpoint.clone(),
            self.bucket.clone(),
            self.prefix.clone(),
        ];
        if let Some(h) = host_of(&self.endpoint) {
            v.push(h);
        }
        v.extend(self.access_key.clone());
        v.extend(self.secret_key.clone());
        Redactor::new(v)
    }
}

fn host_of(endpoint: &str) -> Option<String> {
    let rest = endpoint.split_once("://").map_or(endpoint, |x| x.1);
    let h = rest.split('/').next()?;
    if h.is_empty() { None } else { Some(h.to_string()) }
}

/// Replaces every occurrence of a secret value with `<redacted>`. Values shorter than 3 bytes are ignored (they would
/// mangle ordinary text); the longest value is replaced first so that a secret containing another one is fully removed.
#[derive(Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new(mut secrets: Vec<String>) -> Self {
        secrets.retain(|s| s.len() >= 3);
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Redactor { secrets }
    }

    pub fn from_env() -> Self {
        let mut v = Vec::new();
        for k in [ENV_ENDPOINT, ENV_BUCKET, ENV_PREFIX, ENV_KEY_ID, ENV_SECRET] {
            v.extend(env_nonempty(k));
        }
        if let Some(h) = env_nonempty(ENV_ENDPOINT).as_deref().and_then(host_of) {
            v.push(h);
        }
        Redactor::new(v)
    }

    pub fn apply(&self, s: &str) -> String {
        let mut out = s.to_string();
        for sec in &self.secrets {
            out = out.replace(sec.as_str(), "<redacted>");
        }
        out
    }

    pub fn leaks(&self, s: &str) -> bool {
        self.secrets.iter().any(|x| s.contains(x.as_str()))
    }
}

/// A local-disk store rooted at `dir` (the CPU-bound control arm: same decode path, no network).
pub fn local_store(dir: &std::path::Path) -> Result<Arc<dyn ObjectStore>, String> {
    LocalFileSystem::new_with_prefix(dir)
        .map(|s| Arc::new(s) as Arc<dyn ObjectStore>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redactor_removes_all_values_and_the_longest_first() {
        let r = Redactor::new(vec![
            "my-secret-bucket".into(),
            "storage.example.com".into(),
            "https://storage.example.com".into(),
            "AKIAEXAMPLEKEY".into(),
        ]);
        let msg = "Error performing GET https://storage.example.com/my-secret-bucket/x in 5ms - key AKIAEXAMPLEKEY";
        let out = r.apply(msg);
        assert!(!r.leaks(&out), "{out}");
        assert!(out.contains("<redacted>"));
        // a secret that contains another one must not leave a fragment of the longer one
        assert!(!out.contains("example"), "{out}");
    }

    #[test]
    fn no_redactor_leaks_the_known_value() {
        // the known-bad version: an empty redactor leaves the value in place, and `leaks` on a REAL redactor sees it
        let real = Redactor::new(vec!["supersecretvalue".into()]);
        let bad = Redactor::default();
        let msg = "failed for supersecretvalue";
        assert!(real.leaks(&bad.apply(msg)));
        assert!(!real.leaks(&real.apply(msg)));
    }

    #[test]
    fn debug_of_the_spec_shows_nothing() {
        let s = StoreSpec {
            endpoint: "https://secret-endpoint.example".into(),
            bucket: "secret-bucket".into(),
            prefix: "secret-prefix".into(),
            region: "auto".into(),
            access_key: Some("secret-key".into()),
            secret_key: Some("secret-secret".into()),
            allow_http: false,
            http1_only: false,
            label: "x".into(),
            simulated: false,
        };
        let d = format!("{s:?}");
        assert!(!d.contains("secret"), "{d}");
    }
}
