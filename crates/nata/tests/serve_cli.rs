use std::process::Command;
#[test]
fn serve_help_and_limits() {
    let out = Command::new(env!("CARGO_BIN_EXE_nata"))
        .args(["serve", "--help"])
        .output()
        .expect("help");
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for text in [
        "--listen",
        "--unsafe-listen",
        "--max-connections",
        "--threads",
        "--max-queries",
        "trust authentication",
        "no TLS",
    ] {
        assert!(help.contains(text), "{text}: {help}");
    }
    for (flag, value) in [
        ("--max-connections", "0"),
        ("--max-connections", "65"),
        ("--threads", "0"),
        ("--max-queries", "0"),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_nata"))
            .args(["serve", flag, value])
            .output()
            .expect("invalid config");
        assert_eq!(out.status.code(), Some(2));
    }
    let address =
        std::net::SocketAddr::new(std::net::Ipv4Addr::UNSPECIFIED.into(), 5432).to_string();
    let out = Command::new(env!("CARGO_BIN_EXE_nata"))
        .args(["serve", "--listen", &address])
        .output()
        .expect("unsafe rejected");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--unsafe-listen"));
}
