//! Runtime-selectable Snappy block decoder behind the `snap::raw` API that
//! `parquet` uses. `SNAP_IMPL=orig|cpp` (default `orig`). Every decompress
//! call is timed so the cost on real Parquet pages can be read off directly.

pub use snap_orig::Error;

pub mod raw {
    use super::Error;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::time::Instant;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Impl {
        Orig,
        Cpp,
    }

    pub fn which() -> Impl {
        static W: OnceLock<Impl> = OnceLock::new();
        *W.get_or_init(|| match std::env::var("SNAP_IMPL").as_deref() {
            Ok("cpp") => Impl::Cpp,
            _ => Impl::Orig,
        })
    }

    static NANOS: AtomicU64 = AtomicU64::new(0);
    static BYTES_OUT: AtomicU64 = AtomicU64::new(0);
    static CALLS: AtomicU64 = AtomicU64::new(0);

    /// (nanoseconds in decompress, uncompressed bytes produced, calls)
    pub fn stats() -> (u64, u64, u64) {
        (NANOS.load(Relaxed), BYTES_OUT.load(Relaxed), CALLS.load(Relaxed))
    }

    pub fn reset_stats() {
        NANOS.store(0, Relaxed);
        BYTES_OUT.store(0, Relaxed);
        CALLS.store(0, Relaxed);
    }

    /// With `SNAP_DUMP=<file>`, append the first `SNAP_DUMP_MAX` (default 600)
    /// compressed blocks as `[u32 compressed_len][u32 output_len][bytes]`, so a
    /// standalone benchmark can replay real Parquet pages.
    fn dump(input: &[u8], out_len: usize) {
        use std::io::Write;
        use std::sync::Mutex;
        static FILE: OnceLock<Option<Mutex<(std::fs::File, u64, u64)>>> = OnceLock::new();
        let f = FILE.get_or_init(|| {
            let path = std::env::var("SNAP_DUMP").ok()?;
            let max = std::env::var("SNAP_DUMP_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(600);
            Some(Mutex::new((std::fs::File::create(path).ok()?, 0, max)))
        });
        if let Some(m) = f {
            let mut g = m.lock().expect("dump lock");
            if g.1 < g.2 {
                g.1 += 1;
                let _ = g.0.write_all(&(input.len() as u32).to_le_bytes());
                let _ = g.0.write_all(&(out_len as u32).to_le_bytes());
                let _ = g.0.write_all(input);
            }
        }
    }

    pub struct Decoder {
        orig: snap_orig::raw::Decoder,
    }

    impl Default for Decoder {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Decoder {
        pub fn new() -> Self {
            Decoder { orig: snap_orig::raw::Decoder::new() }
        }

        pub fn decompress(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize, Error> {
            dump(input, output.len());
            let start = Instant::now();
            let result = match which() {
                Impl::Orig => self.orig.decompress(input, output),
                Impl::Cpp => {
                    let mut len = output.len();
                    // SAFETY: pointers and lengths come from live slices; the C API
                    // checks the output capacity against the stored length.
                    let status = unsafe {
                        snappy_src::snappy_uncompress(
                            input.as_ptr().cast(),
                            input.len(),
                            output.as_mut_ptr().cast(),
                            &mut len,
                        )
                    };
                    if status == 0 {
                        Ok(len)
                    } else {
                        // Let the original decoder produce a proper `Error`.
                        self.orig.decompress(input, output)
                    }
                }
            };
            NANOS.fetch_add(start.elapsed().as_nanos() as u64, Relaxed);
            if let Ok(n) = &result {
                BYTES_OUT.fetch_add(*n as u64, Relaxed);
            }
            CALLS.fetch_add(1, Relaxed);
            result
        }
    }

    pub struct Encoder(snap_orig::raw::Encoder);

    impl Default for Encoder {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Encoder {
        pub fn new() -> Self {
            Encoder(snap_orig::raw::Encoder::new())
        }
        pub fn compress(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize, Error> {
            self.0.compress(input, output)
        }
    }

    pub fn decompress_len(input: &[u8]) -> Result<usize, Error> {
        snap_orig::raw::decompress_len(input)
    }

    pub fn max_compress_len(n: usize) -> usize {
        snap_orig::raw::max_compress_len(n)
    }
}
