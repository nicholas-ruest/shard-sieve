use shard_sieve::{inspect_archives, InspectOptions, IssueCode};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tar::{Builder, Header};
use tempfile::TempDir;

fn write_tar(path: &Path, members: &[(&str, &[u8])]) {
    let file = File::create(path).expect("create fixture archive");
    let mut builder = Builder::new(file);
    for (name, payload) in members {
        let mut header = Header::new_gnu();
        header.set_size(payload.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, Cursor::new(*payload))
            .expect("append fixture member");
    }
    builder.finish().expect("finish fixture archive");
}

fn options() -> InspectOptions {
    InspectOptions {
        required_extensions: BTreeSet::from(["jpg".to_owned(), "json".to_owned()]),
        max_skew: 0.5,
        include_hashes: true,
    }
}

#[test]
fn accepts_complete_balanced_shards_and_hashes_payloads() {
    let temp = TempDir::new().expect("temp directory");
    let first = temp.path().join("000.tar");
    let second = temp.path().join("001.tar");
    write_tar(
        &first,
        &[("000.jpg", b"image-a"), ("000.json", b"{\"label\":\"a\"}")],
    );
    write_tar(
        &second,
        &[("001.jpg", b"image-b"), ("001.json", b"{\"label\":\"b\"}")],
    );

    let report = inspect_archives(&[first, second], &options()).expect("inspect fixtures");

    assert!(report.valid);
    assert!(report.issues.is_empty());
    assert_eq!(report.totals.samples, 2);
    assert_eq!(report.totals.members, 4);
    assert!(report.archives[0].samples[0].members[0]
        .sha256
        .as_ref()
        .is_some_and(|hash| hash.len() == 64));
}

#[test]
fn reports_missing_modality_and_empty_payload() {
    let temp = TempDir::new().expect("temp directory");
    let archive = temp.path().join("bad.tar");
    write_tar(&archive, &[("000.jpg", b"")]);

    let report = inspect_archives(&[archive], &options()).expect("inspect fixture");

    assert!(!report.valid);
    let codes = report
        .issues
        .iter()
        .map(|issue| issue.code)
        .collect::<BTreeSet<_>>();
    assert!(codes.contains(&IssueCode::EmptyPayload));
    assert!(codes.contains(&IssueCode::MissingExtension));
}

#[test]
fn reports_sample_keys_repeated_across_shards() {
    let temp = TempDir::new().expect("temp directory");
    let paths: Vec<PathBuf> = ["000.tar", "001.tar"]
        .into_iter()
        .map(|name| temp.path().join(name))
        .collect();
    for path in &paths {
        write_tar(path, &[("same.jpg", b"x"), ("same.json", b"{}")]);
    }

    let report = inspect_archives(&paths, &options()).expect("inspect fixtures");

    assert!(report
        .issues
        .iter()
        .any(|issue| issue.code == IssueCode::CrossShardDuplicate));
}

#[test]
fn reports_excessive_sample_count_skew() {
    let temp = TempDir::new().expect("temp directory");
    let small = temp.path().join("small.tar");
    let large = temp.path().join("large.tar");
    write_tar(&small, &[("a.jpg", b"a")]);
    write_tar(&large, &[("b.jpg", b"b"), ("c.jpg", b"c"), ("d.jpg", b"d")]);
    let inspect_options = InspectOptions {
        max_skew: 0.25,
        ..InspectOptions::default()
    };

    let report = inspect_archives(&[small, large], &inspect_options).expect("inspect fixtures");

    assert!(report
        .issues
        .iter()
        .any(|issue| issue.code == IssueCode::ShardSkew));
}
