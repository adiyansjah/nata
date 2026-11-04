//! DuckDB as the reference: the same query over the same files in the same store (through its `httpfs` extension),
//! and as the oracle for the expected result (on the local base file).
//!
//! HOW DuckDB reads the store: `INSTALL httpfs; LOAD httpfs;` (installed once before the timing starts), then
//! `SET s3_endpoint='<host[:port]>'; SET s3_url_style='path'; SET s3_use_ssl=<false for the simulated store, true for the
//! real one>; SET s3_region=...;` and, only when HMAC credentials are configured, `CREATE SECRET (TYPE S3, ...)`.
//! Files are named explicitly (`read_parquet([...])`), so DuckDB sends no LIST request. Each run is a NEW duckdb process
//! (no cache carries over); the time reported is the CLI's own `.timer` for the one query statement, which excludes process
//! start, extension load and the SETs, and includes DuckDB's planning (footer reads) and execution. The script reaches
//! duckdb on stdin, so credentials never appear in an argument list, a file or a log.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::exec::{QuerySpec, normalize};
use crate::layouts::rel_key;
use crate::store::{Redactor, StoreSpec};

#[derive(Clone)]
pub struct DuckCfg {
    /// `host[:port]`, no scheme.
    pub endpoint: String,
    pub use_ssl: bool,
    pub bucket: String,
    pub prefix: String,
    pub region: String,
    pub key: Option<String>,
    pub secret: Option<String>,
    pub threads: usize,
    pub redact: Redactor,
}

pub struct DuckRun {
    pub text: String,
    pub seconds: f64,
}

impl DuckCfg {
    pub fn from_spec(spec: &StoreSpec, threads: usize) -> DuckCfg {
        let (scheme_ssl, rest) = match spec.endpoint.split_once("://") {
            Some(("http", r)) => (false, r),
            Some((_, r)) => (true, r),
            None => (true, spec.endpoint.as_str()),
        };
        DuckCfg {
            endpoint: rest.trim_end_matches('/').to_string(),
            use_ssl: scheme_ssl,
            bucket: spec.bucket.clone(),
            prefix: spec.prefix.clone(),
            region: spec.region.clone(),
            key: spec.access_key.clone(),
            secret: spec.secret_key.clone(),
            threads,
            redact: spec.redactor(),
        }
    }

    fn url(&self, rel: &str) -> String {
        if self.prefix.is_empty() {
            format!("s3://{}/{}", self.bucket, rel)
        } else {
            format!("s3://{}/{}/{}", self.bucket, self.prefix, rel)
        }
    }

    fn script(&self, layout: &str, files: usize, spec: &QuerySpec) -> String {
        let list: Vec<String> = (0..files).map(|i| format!("'{}'", self.url(&rel_key(layout, i)))).collect();
        let table = format!("read_parquet([{}])", list.join(", "));
        let mut s = String::new();
        s.push_str("LOAD httpfs;\n");
        s.push_str(&format!("SET threads={};\n", self.threads));
        s.push_str(&format!("SET s3_endpoint='{}';\n", self.endpoint));
        s.push_str("SET s3_url_style='path';\n");
        s.push_str(&format!("SET s3_use_ssl={};\n", self.use_ssl));
        s.push_str(&format!("SET s3_region='{}';\n", self.region));
        if let (Some(k), Some(sec)) = (&self.key, &self.secret) {
            s.push_str(&format!(
                "CREATE OR REPLACE SECRET nata_s3 (TYPE S3, KEY_ID '{k}', SECRET '{sec}', ENDPOINT '{}', URL_STYLE 'path', USE_SSL {}, REGION '{}');\n",
                self.endpoint, self.use_ssl, self.region
            ));
        }
        s.push_str(".timer on\n");
        s.push_str(&spec.duck_sql.replace("{T}", &table));
        s.push_str(";\n");
        s
    }

    /// One cold run of `spec` over the files of `layout` in the store.
    pub fn run(&self, layout: &str, files: usize, spec: &QuerySpec) -> Result<DuckRun, String> {
        run_duckdb(&self.script(layout, files, spec), &self.redact)
    }

    /// `INSTALL httpfs` (needs network access to DuckDB's extension repository); run once before timing.
    pub fn install_httpfs() -> Result<(), String> {
        run_duckdb("INSTALL httpfs;\n", &Redactor::default()).map(|_| ())
    }
}

fn run_duckdb(script: &str, redact: &Redactor) -> Result<DuckRun, String> {
    let mut child = Command::new("duckdb")
        .args(["-bail", "-list", "-noheader", "-separator", "|", "-nullvalue", "NULL"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start duckdb: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(script.as_bytes())
        .map_err(|e| format!("writing to duckdb: {e}"))?;
    let out = child.wait_with_output().map_err(|e| format!("waiting for duckdb: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() || stderr.contains("Error") {
        return Err(redact.apply(&format!("duckdb failed ({}): {} {}", out.status, stderr.trim(), stdout.trim())));
    }
    parse_output(&stdout)
}

/// Split the CLI output into the result lines and the `Run Time (s): real X ...` line.
pub fn parse_output(stdout: &str) -> Result<DuckRun, String> {
    let mut lines = Vec::new();
    let mut seconds = None;
    for l in stdout.lines() {
        if let Some(rest) = l.strip_prefix("Run Time (s):") {
            let mut it = rest.split_whitespace();
            while let Some(w) = it.next() {
                if w == "real" {
                    seconds = it.next().and_then(|x| x.parse::<f64>().ok());
                }
            }
        } else if !l.is_empty() {
            lines.push(l);
        }
    }
    let seconds = seconds.ok_or("no 'Run Time' line in the duckdb output")?;
    Ok(DuckRun {
        text: normalize(&lines.join("\n")),
        seconds,
    })
}

/// The oracle: `spec` over local files (any `read_parquet` argument), normalised.
pub fn expected_local(table_sql: &str, spec: &QuerySpec) -> Result<String, String> {
    if spec.limit.is_some() {
        return Ok(format!("{} row(s)\n", spec.limit.unwrap_or(0)));
    }
    let script = format!("{};\n", spec.duck_sql.replace("{T}", table_sql));
    let mut child = Command::new("duckdb")
        .args(["-bail", "-list", "-noheader", "-separator", "|", "-nullvalue", "NULL"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start duckdb: {e}"))?;
    child.stdin.take().ok_or("no stdin")?.write_all(script.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("duckdb failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(normalize(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_cli_timer_line() {
        let r = parse_output("3|10\n1|2\nRun Time (s): real 0.123 user 0.100 sys 0.010\n").unwrap();
        assert_eq!(r.text, "1|2\n3|10\n");
        assert!((r.seconds - 0.123).abs() < 1e-9);
        assert!(parse_output("1|2\n").is_err());
    }

    #[test]
    fn the_script_names_files_and_carries_no_secret_when_anonymous() {
        let spec = StoreSpec::sim("127.0.0.1:9", "bkt");
        let c = DuckCfg::from_spec(&spec, 4);
        let s = c.script("small1", 2, &crate::exec::Q_SUM);
        assert!(s.contains("s3://bkt/small1/part-0000.parquet") && s.contains("s3://bkt/small1/part-0001.parquet"));
        assert!(s.contains("SET s3_use_ssl=false") && s.contains("s3_endpoint='127.0.0.1:9'"));
        assert!(!s.contains("SECRET"));
        assert!(s.find(".timer on").unwrap() < s.find("SELECT count").unwrap());
    }
}
