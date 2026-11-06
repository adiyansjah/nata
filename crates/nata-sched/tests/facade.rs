//! The loom models explore the code that runs only if every synchronisation primitive of the crate comes from `src/sync.rs`. This test reads the sources
//! and fails if another file names `std::sync`, `std::thread` or an atomic directly. (Shown to fail: the mutant `facade-bypassed-by-a-std-atomic`.)

#[test]
fn only_the_facade_names_std_synchronisation() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in std::fs::read_dir(&src).expect("src dir") {
        let path = entry.expect("entry").path();
        if path.file_name().is_some_and(|n| n == "sync.rs")
            || path.extension().is_none_or(|e| e != "rs")
        {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        checked += 1;
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for banned in [
                "std::sync::atomic",
                "std::thread::spawn",
                "std::thread::Builder",
                "std::sync::Mutex",
                "std::sync::Condvar",
                "std::sync::Arc",
                "std::sync::mpsc",
            ] {
                assert!(
                    !code.contains(banned),
                    "{}:{} uses {banned} directly; go through crate::sync",
                    path.display(),
                    i + 1
                );
            }
        }
    }
    assert!(checked >= 1);
}
