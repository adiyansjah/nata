//! object_store with the local file system backend: put a large object, read it back whole, by range, and with many
//! concurrent range reads from a tokio runtime (the shape of a Parquet scan). No network.
use bytes::Bytes;
use object_store::{local::LocalFileSystem, path::Path, ObjectStore};
use spike_crates_check::test_data;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn put_get_and_many_concurrent_range_reads() {
    let dir = std::env::temp_dir().join(format!("crates-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(LocalFileSystem::new_with_prefix(&dir).unwrap());
    let data = Bytes::from(test_data(96 << 20, 7));
    let path = Path::from("big.bin");
    store.put(&path, data.clone().into()).await.unwrap();

    let meta = store.head(&path).await.unwrap();
    assert_eq!(meta.size as usize, data.len());
    let whole = store.get(&path).await.unwrap().bytes().await.unwrap();
    assert!(whole == data, "whole-object read differs");

    let mut tasks = Vec::new();
    for i in 0..64u64 {
        let (store, path, data) = (store.clone(), path.clone(), data.clone());
        tasks.push(tokio::spawn(async move {
            let start = (i * 1_234_567) % (data.len() as u64 - 1_000_000);
            let end = start + 1_000_000;
            let got = store.get_range(&path, start..end).await.unwrap();
            assert!(got[..] == data[start as usize..end as usize], "range {start}..{end} differs");
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    store.delete(&path).await.unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_missing_object_is_a_not_found_error() {
    let dir = std::env::temp_dir().join(format!("crates-check-nf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = LocalFileSystem::new_with_prefix(&dir).unwrap();
    let e = store.get(&Path::from("nope")).await.unwrap_err();
    assert!(matches!(e, object_store::Error::NotFound { .. }), "unexpected error: {e}");
    std::fs::remove_dir_all(&dir).ok();
}
