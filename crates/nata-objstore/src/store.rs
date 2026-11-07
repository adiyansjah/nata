//! Locations (`s3://bucket/key`, `gs://bucket/key`), the stores behind them and their credentials.
//!
//! Credentials come from the environment and the instance metadata ONLY (ADR 0011): static keys, a named profile of the shared credentials file,
//! web identity / container credentials, and the instance metadata service for S3; a service-account key or application-default file, then the
//! metadata server for GCS. No SQL credentials, no command-line secrets. Nothing here formats a key, a token, an endpoint or a bucket into an error:
//! every message is a fixed text or names the scheme and the problem.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey};
use object_store::gcp::{GoogleCloudStorageBuilder, GoogleConfigKey};
use object_store::path::Path;
use object_store::{ClientOptions, ObjectStore};

use crate::policy::{Config, Mutants};
use crate::remote::{BoundedConnector, ClientSettings};
use crate::util::lock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scheme {
    S3,
    Gcs,
}

impl Scheme {
    pub fn name(self) -> &'static str {
        match self {
            Scheme::S3 => "s3",
            Scheme::Gcs => "gs",
        }
    }
}

/// A parsed `s3://bucket/key` or `gs://bucket/key`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub scheme: Scheme,
    pub bucket: String,
    pub key: String,
}

impl Location {
    pub fn parse(text: &str) -> Result<Location, String> {
        let (scheme, rest) = if let Some(r) = text.strip_prefix("s3://") {
            (Scheme::S3, r)
        } else if let Some(r) = text.strip_prefix("gs://") {
            (Scheme::Gcs, r)
        } else {
            return Err("a location must start with s3:// or gs://".into());
        };
        let (bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
        if bucket.is_empty()
            || bucket.len() > 255
            || bucket
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        {
            return Err(format!(
                "the {} location has no valid bucket name",
                scheme.name()
            ));
        }
        if key.len() > 1024 || key.contains('\0') {
            return Err(format!(
                "the {} object name is too long or malformed",
                scheme.name()
            ));
        }
        Ok(Location {
            scheme,
            bucket: bucket.to_string(),
            key: key.to_string(),
        })
    }

    /// The object path inside the bucket. Names with empty or relative segments cannot be addressed the way the stores' APIs expect.
    pub fn path(&self) -> Result<Path, String> {
        if self.key.is_empty() {
            return Err(format!(
                "the {} location names a bucket, not an object",
                self.scheme.name()
            ));
        }
        Path::parse(&self.key).map_err(|_| {
            format!(
                "the {} object name has an empty or relative segment",
                self.scheme.name()
            )
        })
    }

    pub fn display(&self) -> String {
        format!("{}://{}/{}", self.scheme.name(), self.bucket, self.key)
    }
}

/// S3 settings: the AWS environment (`AWS_*` names the builder knows), or whatever a test passes. A secret lives in `config` only until the builder
/// takes it; `Debug` shows keys, never values.
#[derive(Clone, Default)]
pub struct S3Options {
    pub config: Vec<(AmazonS3ConfigKey, String)>,
    /// The profile of the shared credentials file to read static keys from (`AWS_PROFILE`), and the file (`AWS_SHARED_CREDENTIALS_FILE`, else `~/.aws/credentials`).
    pub profile: Option<String>,
    pub credentials_file: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for S3Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Options")
            .field(
                "keys",
                &self
                    .config
                    .iter()
                    .map(|(k, _)| k.as_ref().to_string())
                    .collect::<Vec<_>>(),
            )
            .field("profile", &self.profile.is_some())
            .finish()
    }
}

impl S3Options {
    /// From the process environment: every `AWS_*` variable the object_store builder knows (keys, token, region, endpoint, `AWS_ALLOW_HTTP`, web identity,
    /// container credentials, metadata endpoint), plus `AWS_PROFILE` and `AWS_SHARED_CREDENTIALS_FILE`.
    pub fn from_env() -> S3Options {
        let mut o = S3Options::default();
        for (k, v) in std::env::vars() {
            if v.is_empty() {
                continue;
            }
            match k.as_str() {
                "AWS_PROFILE" => o.profile = Some(v),
                "AWS_SHARED_CREDENTIALS_FILE" => o.credentials_file = Some(v.into()),
                "AWS_DEFAULT_REGION" => o.config.push((AmazonS3ConfigKey::Region, v)),
                "AWS_ENDPOINT_URL" | "AWS_ENDPOINT_URL_S3" => {
                    o.config.push((AmazonS3ConfigKey::Endpoint, v))
                }
                "AWS_EC2_METADATA_SERVICE_ENDPOINT" => {
                    o.config.push((AmazonS3ConfigKey::MetadataEndpoint, v))
                }
                _ => {
                    if let Some(rest) = k.strip_prefix("AWS_")
                        && let Ok(key) = format!("aws_{}", rest.to_ascii_lowercase())
                            .parse::<AmazonS3ConfigKey>()
                    {
                        o.config.push((key, v));
                    }
                }
            }
        }
        o
    }

    fn has(&self, key: AmazonS3ConfigKey) -> bool {
        self.config.iter().any(|(k, _)| *k == key)
    }

    fn get(&self, key: AmazonS3ConfigKey) -> Option<&str> {
        self.config
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The identity of the credential source (never the secret): the access key id of static keys, else the chain in use.
    pub fn scope(&self) -> String {
        if let Some(id) = self.get(AmazonS3ConfigKey::AccessKeyId) {
            return format!("key:{}", fnv(id));
        }
        if let Some(p) = &self.profile {
            return format!("profile:{}", fnv(p));
        }
        if self.has(AmazonS3ConfigKey::WebIdentityTokenFile) {
            return "web-identity".into();
        }
        if self.has(AmazonS3ConfigKey::ContainerCredentialsRelativeUri)
            || self.has(AmazonS3ConfigKey::ContainerCredentialsFullUri)
        {
            return "container".into();
        }
        "instance".into()
    }
}

/// GCS settings: the `GOOGLE_*` environment (`GOOGLE_SERVICE_ACCOUNT`, `GOOGLE_APPLICATION_CREDENTIALS`, `GOOGLE_BASE_URL` ...), else the metadata server.
#[derive(Clone, Default)]
pub struct GcsOptions {
    pub config: Vec<(GoogleConfigKey, String)>,
    /// Allow a plain-http base URL (a local test endpoint).
    pub allow_http: bool,
}

impl std::fmt::Debug for GcsOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcsOptions")
            .field(
                "keys",
                &self
                    .config
                    .iter()
                    .map(|(k, _)| k.as_ref().to_string())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl GcsOptions {
    pub fn from_env() -> GcsOptions {
        let mut o = GcsOptions::default();
        for (k, v) in std::env::vars() {
            if v.is_empty() {
                continue;
            }
            if k == "GOOGLE_ALLOW_HTTP" {
                o.allow_http = v.eq_ignore_ascii_case("true") || v == "1";
            } else if let Some(rest) = k.strip_prefix("GOOGLE_")
                && let Ok(key) =
                    format!("google_{}", rest.to_ascii_lowercase()).parse::<GoogleConfigKey>()
            {
                o.config.push((key, v));
            }
        }
        o
    }

    pub fn scope(&self) -> String {
        for (k, v) in &self.config {
            match k {
                GoogleConfigKey::ServiceAccount
                | GoogleConfigKey::ApplicationCredentials
                | GoogleConfigKey::ServiceAccountKey => return format!("key:{}", fnv(v)),
                _ => {}
            }
        }
        "metadata".into()
    }
}

/// FNV-1a, hex: an opaque, stable identifier of a configuration string for cache keys (not a security primitive; the input is never a secret).
fn fnv(s: &str) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Static keys read from a profile of the shared credentials file (an INI file). Bounded: at most 64 KiB are read. Only static keys are understood
/// (`credential_process`, SSO and role chaining are not: the profile must hold keys).
pub struct ProfileKeys {
    pub key_id: String,
    pub secret: String,
    pub token: Option<String>,
    pub region: Option<String>,
}

impl std::fmt::Debug for ProfileKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileKeys").finish_non_exhaustive()
    }
}

const PROFILE_FILE_CAP: u64 = 64 << 10;

pub fn read_profile(path: &std::path::Path, profile: &str) -> Result<ProfileKeys, String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(PROFILE_FILE_CAP + 1).read_to_string(&mut text))
        .map_err(|_| "cannot read the shared credentials file".to_string())?;
    if text.len() as u64 > PROFILE_FILE_CAP {
        return Err("the shared credentials file is too large".into());
    }
    parse_profile(&text, profile)
}

pub fn parse_profile(text: &str, profile: &str) -> Result<ProfileKeys, String> {
    let mut in_profile = false;
    let mut found = false;
    let mut values: HashMap<String, String> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let name = name.trim();
            let name = name.strip_prefix("profile ").map_or(name, str::trim);
            in_profile = name == profile;
            found |= in_profile;
            continue;
        }
        if in_profile && let Some((k, v)) = line.split_once('=') {
            values.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    if !found {
        return Err("the named profile is not in the shared credentials file".into());
    }
    let take = |k: &str| values.get(k).filter(|v| !v.is_empty()).cloned();
    match (take("aws_access_key_id"), take("aws_secret_access_key")) {
        (Some(key_id), Some(secret)) => Ok(ProfileKeys { key_id, secret, token: take("aws_session_token"), region: take("region") }),
        _ => Err("the named profile holds no static access keys (credential_process, SSO and role chaining are not supported)".into()),
    }
}

fn client_options(cfg: &Config, m: &Mutants, allow_http: bool) -> ClientOptions {
    let mut co = ClientOptions::new()
        .with_pool_max_idle_per_host(64)
        .with_connect_timeout(Duration::from_secs(5))
        .with_allow_http(allow_http);
    // The per-attempt timeout is ours (it must be able to fail as a mutant); the client's own timeout is only a far backstop.
    co = if m.no_attempt_timeout() {
        co.with_timeout_disabled()
    } else {
        co.with_timeout(cfg.op_deadline + Duration::from_secs(10))
    };
    if allow_http {
        co = co.with_http1_only();
    }
    co
}

fn no_retry() -> object_store::RetryConfig {
    object_store::RetryConfig {
        backoff: object_store::BackoffConfig::default(),
        max_retries: 0,
        retry_timeout: Duration::from_secs(1),
    }
}

/// The connector of EVERY store (and so of every HTTP client `object_store` makes for it: reads, LIST, metadata and token services). It builds the reqwest
/// client itself so that it can set `redirect::Policy::none()`: a redirect from the endpoint is never followed (`x-amz-security-token` and other custom
/// credential headers would travel to the new origin). Read paths never need one: the endpoint and the addressing style are configured explicitly.
fn connector(cfg: &Config, m: &Mutants, allow_http: bool) -> BoundedConnector {
    BoundedConnector {
        cap: cfg.error_body_cap,
        meta_cap: cfg.meta_body_cap,
        bound_success: !m.no_success_body_bound(),
        client: ClientSettings {
            connect_timeout: Duration::from_secs(5),
            // The per-attempt timeout is ours (it must be able to fail as a mutant); the client's own timeout is only a far backstop.
            timeout: if m.no_attempt_timeout() {
                None
            } else {
                Some(cfg.op_deadline + Duration::from_secs(10))
            },
            pool_max_idle_per_host: 64,
            allow_http,
            follow_redirects: m.default_redirects(),
        },
    }
}

pub fn build_s3(
    o: &S3Options,
    bucket: &str,
    cfg: &Config,
    m: &Mutants,
) -> Result<Arc<dyn ObjectStore>, String> {
    let allow_http = o
        .get(AmazonS3ConfigKey::Client(
            object_store::ClientConfigKey::AllowHttp,
        ))
        .is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1");
    let mut b = AmazonS3Builder::new()
        .with_bucket_name(bucket)
        .with_client_options(client_options(cfg, m, allow_http))
        .with_retry(no_retry());
    for (k, v) in &o.config {
        b = b.with_config(*k, v.clone());
    }
    // a named profile supplies static keys unless the environment already gave keys (explicit keys win)
    if let (Some(profile), false) = (&o.profile, o.has(AmazonS3ConfigKey::AccessKeyId)) {
        let path = match &o.credentials_file {
            Some(p) => p.clone(),
            None => std::env::var_os("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".aws").join("credentials"))
                .ok_or("no home directory for the shared credentials file")?,
        };
        let keys = read_profile(&path, profile)?;
        b = b
            .with_access_key_id(keys.key_id)
            .with_secret_access_key(keys.secret);
        if let Some(t) = keys.token {
            b = b.with_token(t);
        }
        if let (Some(r), false) = (keys.region, o.has(AmazonS3ConfigKey::Region)) {
            b = b.with_region(r);
        }
    }
    if !o.has(AmazonS3ConfigKey::Region) && o.profile.is_none() {
        b = b.with_region("us-east-1");
    }
    if !m.no_transport_bound() {
        b = b.with_http_connector(connector(cfg, m, allow_http));
    }
    b.build()
        .map(|s| Arc::new(s) as Arc<dyn ObjectStore>)
        .map_err(|_| {
            "cannot build the S3 store (check the AWS environment; details withheld)".to_string()
        })
}

pub fn build_gcs(
    o: &GcsOptions,
    bucket: &str,
    cfg: &Config,
    m: &Mutants,
) -> Result<Arc<dyn ObjectStore>, String> {
    build_gcs_with(o, bucket, cfg, m, None)
}

/// [`build_gcs`] with a given credential provider instead of the environment chain (the tests give the store a static bearer token, which the
/// fake endpoint, being anonymous, would not otherwise make the client send).
pub fn build_gcs_with(
    o: &GcsOptions,
    bucket: &str,
    cfg: &Config,
    m: &Mutants,
    credentials: Option<GcsCredentials>,
) -> Result<Arc<dyn ObjectStore>, String> {
    let mut b = GoogleCloudStorageBuilder::new()
        .with_bucket_name(bucket)
        .with_client_options(client_options(cfg, m, o.allow_http))
        .with_retry(no_retry());
    for (k, v) in &o.config {
        b = b.with_config(*k, v.clone());
    }
    if !m.no_transport_bound() {
        b = b.with_http_connector(connector(cfg, m, o.allow_http));
    }
    if let Some(c) = credentials {
        b = b.with_credentials(c);
    }
    b.build()
        .map(|s| Arc::new(s) as Arc<dyn ObjectStore>)
        .map_err(|_| {
            "cannot build the GCS store (check the Google environment; details withheld)"
                .to_string()
        })
}

pub type GcsCredentials =
    Arc<dyn object_store::CredentialProvider<Credential = object_store::gcp::GcpCredential>>;

/// A store and the scope its credentials give (for cache keys).
#[derive(Clone)]
pub struct StoreEntry {
    pub store: Arc<dyn ObjectStore>,
    pub scope: String,
}

/// The stores of a node, built on first use from the environment captured at construction.
pub struct Stores {
    s3: S3Options,
    gcs: GcsOptions,
    cfg: Config,
    mutants: Mutants,
    map: Mutex<HashMap<(Scheme, String), StoreEntry>>,
}

impl Stores {
    pub fn new(s3: S3Options, gcs: GcsOptions, cfg: Config, mutants: Mutants) -> Stores {
        Stores {
            s3,
            gcs,
            cfg,
            mutants,
            map: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, loc: &Location) -> Result<StoreEntry, String> {
        let key = (loc.scheme, loc.bucket.clone());
        if let Some(e) = lock(&self.map).get(&key) {
            return Ok(e.clone());
        }
        let entry = match loc.scheme {
            Scheme::S3 => StoreEntry {
                store: build_s3(&self.s3, &loc.bucket, &self.cfg, &self.mutants)?,
                scope: self.s3.scope(),
            },
            Scheme::Gcs => StoreEntry {
                store: build_gcs(&self.gcs, &loc.bucket, &self.cfg, &self.mutants)?,
                scope: self.gcs.scope(),
            },
        };
        lock(&self.map).insert(key, entry.clone());
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_parse_and_refuse() {
        let l = Location::parse("s3://bkt/dir/a.parquet").expect("s3");
        assert_eq!(
            (l.scheme, l.bucket.as_str(), l.key.as_str()),
            (Scheme::S3, "bkt", "dir/a.parquet")
        );
        assert_eq!(Location::parse("gs://b/x").expect("gs").scheme, Scheme::Gcs);
        for bad in ["http://b/x", "s3://", "s3:///x", "s3://b c/x", "gs://b/\0"] {
            assert!(Location::parse(bad).is_err(), "{bad:?}");
        }
        assert!(
            Location::parse("s3://bkt")
                .expect("bucket only")
                .path()
                .is_err()
        );
        assert!(
            Location::parse("s3://bkt/a//b")
                .expect("parsed")
                .path()
                .is_err(),
            "an empty segment cannot be addressed"
        );
        assert!(
            Location::parse("s3://bkt/a/../b")
                .expect("parsed")
                .path()
                .is_err(),
            "a relative segment cannot be addressed"
        );
    }

    #[test]
    fn profiles_read_static_keys_only() {
        let text = "[default]\naws_access_key_id = AKIDDEFAULT\naws_secret_access_key = s1\n\n# c\n[profile team]\naws_access_key_id=AKIDTEAM\naws_secret_access_key=s2\naws_session_token=tok\nregion=eu-west-1\n[sso]\nsso_session = x\n";
        let d = parse_profile(text, "default").expect("default");
        assert_eq!(
            (d.key_id.as_str(), d.secret.as_str(), d.token),
            ("AKIDDEFAULT", "s1", None)
        );
        let t = parse_profile(text, "team").expect("team");
        assert_eq!(
            (t.key_id.as_str(), t.token.as_deref(), t.region.as_deref()),
            ("AKIDTEAM", Some("tok"), Some("eu-west-1"))
        );
        assert!(parse_profile(text, "nope").is_err());
        assert!(
            parse_profile(text, "sso").is_err(),
            "a profile without static keys is refused, not guessed"
        );
        let e = parse_profile(text, "nope").expect_err("missing");
        assert!(
            !e.contains("AKID") && !e.contains("s1"),
            "errors never print keys: {e}"
        );
    }

    #[test]
    fn scopes_identify_the_source_never_the_secret() {
        let mut o = S3Options::default();
        assert_eq!(o.scope(), "instance");
        o.config
            .push((AmazonS3ConfigKey::AccessKeyId, "AKIDX".into()));
        o.config
            .push((AmazonS3ConfigKey::SecretAccessKey, "TOPSECRET".into()));
        let s = o.scope();
        assert!(
            s.starts_with("key:") && !s.contains("TOPSECRET") && !s.contains("AKIDX"),
            "{s}"
        );
        assert!(
            !format!("{o:?}").contains("TOPSECRET"),
            "Debug must not show values"
        );
    }
}
