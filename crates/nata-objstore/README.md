# nata-objstore

Bounded reads of Parquet objects from S3, MinIO and GCS (card 0006, [ADR 0011](../../docs/decisions/0011-object-store-io.md), [ADR 0012](../../docs/decisions/0012-metadata-cache.md)).
It is the only crate that depends on `object_store`; the Parquet scan sees it through `nata_parquet::RemoteIo`.

| module | job |
|---|---|
| `remote` | every request: per-query (16) and per-node (32) attempt caps, memory grants reserved before the request, identity pinning (version, else ETag and length) with `If-Match`, chunk-by-chunk length checks, 4 attempts with equal-jitter backoff, 30 s per attempt, 120 s per read, `Retry-After` inside the deadline, cancel at every wait. `BoundedConnector` bounds the library's own transport: 4 KiB of an error body, 8 MiB of any success body that is not a ranged object read (LIST pages, credential and token documents) |
| `cache` | the parsed-footer cache: key with provider, credential scope, object, identity and reader version; charged by parsed size; 64 MiB, 1,024 entries, 16 MiB per entry; LRU among unpinned entries; pinned bytes stay charged; a footer that does not fit is bypassed and held under the query's grant |
| `plan` | range merging inside ONE row group (gap 1 MiB, response 64 MiB, 25 % extra); never across row groups |
| `store` | `s3://` and `gs://` locations; credentials from the environment and instance metadata only (static keys, a profile with static keys, web identity, container credentials, IMDSv2; `GOOGLE_*`, the metadata server) |
| `node` | the I/O runtime (two threads), the node, and the handle a scan worker waits on (`ObjectHandle`, `LazyObjectIo`) |

Errors carry a class and attempt count, never the library's text (it can hold an endpoint, a key or a body). Mutation switches (`policy::Mutants`) exist only with
`--features mutants`; `cargo run -p nata-objstore --features mutants --release --bin objstore-sweep -- mutants` shows every test fails on its known-bad version.
The fake endpoint the tests use is `nata-objfake` (loopback, hostile on request).
