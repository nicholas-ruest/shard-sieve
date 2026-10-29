use std::fs::File;
use std::io::Cursor;
use std::process::Command;
use tar::{Builder, Header};
use tempfile::TempDir;

#[test]
fn json_mode_returns_validation_exit_code_and_stable_issue_code() {
    let temp = TempDir::new().expect("temp directory");
    let path = temp.path().join("incomplete.tar");
    let file = File::create(&path).expect("create fixture archive");
    let mut builder = Builder::new(file);
    let mut header = Header::new_gnu();
    header.set_size(3);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "item.jpg", Cursor::new(b"img"))
        .expect("append member");
    builder.finish().expect("finish fixture archive");

    let output = Command::new(env!("CARGO_BIN_EXE_shard-sieve"))
        .args([
            path.to_str().expect("UTF-8 path"),
            "--require",
            "jpg,json",
            "--format",
            "json",
        ])
        .output()
        .expect("run shard-sieve");

    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
    assert!(stdout.contains("\"valid\": false"));
    assert!(stdout.contains("\"missing_extension\""));
}
