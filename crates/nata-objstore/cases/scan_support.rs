//! A scan rig: the real engine (`nata-sql`, `nata-exec`) over the fake object store AND the identical local files, so a test can compare their rows.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use nata_exec::output::{Format, ResultWriter};
use nata_exec::{ExecConfig, execute};
use nata_objfake::FakeS3;
use nata_objstore::{Config, NodeOptions, ObjectHandle};
use nata_parquet::{ErrorKind, RemoteIo, ScanCancel, ScanError};
use nata_sql::{BindOptions, FsSource, RoutedSource, plan_sql_with};

use super::fault::fake_s3;

#[derive(Clone, Default)]
pub struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("buffer").extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct ScanRig {
    pub rt: tokio::runtime::Runtime,
    pub fake: FakeS3,
    pub io: Arc<dyn RemoteIo>,
    pub handle: ObjectHandle,
    pub dir: PathBuf,
}

impl ScanRig {
    pub fn new(name: &str) -> ScanRig {
        ScanRig::with(name, Config::fast_tests(), true)
    }

    pub fn with(name: &str, cfg: Config, cache: bool) -> ScanRig {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        let fake = rt.block_on(FakeS3::start("bkt"));
        let mut o = NodeOptions::from_env();
        o.s3 = fake_s3(&fake);
        o.cfg = cfg;
        o.cache = cache;
        let handle = ObjectHandle::new(o).expect("node");
        let dir = std::env::temp_dir().join(format!("nata-objstore-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        ScanRig {
            rt,
            fake,
            io: Arc::new(handle.clone()),
            handle,
            dir,
        }
    }

    /// The same bytes as a local file and as an object.
    pub fn put(&self, key: &str, bytes: Vec<u8>) {
        let path = self.dir.join(key);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(path, &bytes).expect("write");
        self.fake.put(key, bytes);
    }

    pub fn local(&self, sql: &str) -> String {
        self.sql(&sql.replace("{d}", &self.dir.to_string_lossy()), None)
            .expect("local run")
    }

    pub fn remote(&self, sql: &str) -> Result<String, ScanError> {
        self.sql(&sql.replace("{d}", "s3://bkt"), None)
    }

    /// The rows of the query over the local files, or the error text.
    pub fn local_result(&self, sql: &str) -> Result<String, String> {
        self.sql(&sql.replace("{d}", &self.dir.to_string_lossy()), None)
            .map_err(|e| e.to_string())
    }

    /// The rows of the query over the same objects (`{d}` is `s3://bkt`), or the error text.
    pub fn remote_result(&self, sql: &str) -> Result<String, String> {
        self.sql(&sql.replace("{d}", "s3://bkt"), None)
            .map_err(|e| e.to_string())
    }

    pub fn sql(&self, sql: &str, cancel: Option<&ScanCancel>) -> Result<String, ScanError> {
        let stop = || cancel.is_some_and(ScanCancel::is_cancelled);
        let routed = RoutedSource {
            local: &FsSource,
            remote: self.io.as_ref(),
            cancel: &stop,
        };
        let planned = plan_sql_with(sql, &routed, BindOptions::default())
            .map_err(|e| ScanError::new(ErrorKind::Usage(e.to_string())))?;
        let names: Vec<String> = planned.output.iter().map(|c| c.name.clone()).collect();
        let types: Vec<_> = planned.output.iter().map(|c| c.ty).collect();
        let out = Buf::default();
        let mut writer = ResultWriter::new(out.clone(), Format::Csv, names, &types);
        let mut cfg = ExecConfig::new(4);
        cfg.remote = Some(Arc::clone(&self.io));
        execute(Arc::new(planned.plan), &cfg, cancel, |b| {
            writer.write_batch(b)
        })?;
        writer.finish()?;
        let text = String::from_utf8(out.0.lock().expect("buffer").clone()).expect("utf8");
        Ok(text)
    }
}

impl Drop for ScanRig {
    fn drop(&mut self) {
        let _ = &self.rt;
        std::fs::remove_dir_all(&self.dir).ok();
    }
}
