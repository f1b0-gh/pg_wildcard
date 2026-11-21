// SPDX-License-Identifier: Apache-2.0
use std::{collections::BTreeMap, process::Command};
use wildcard_puffin::puffin::{Blob, Codec, PuffinWriter};
#[test]
fn cli_inspects_aggregate_and_rejects_flags_and_invalid_files() {
    let tmp = std::env::temp_dir().join(format!("wc-segment-budget-{}", std::process::id()));
    std::fs::create_dir(&tmp).unwrap();
    let path = tmp.join("own.puffin");
    let mut w = PuffinWriter::new();
    w.add(
        Blob {
            blob_type: wildcard_puffin::ngram::BLOB_TYPE.into(),
            fields: vec![1],
            snapshot_id: 1,
            sequence_number: 1,
            properties: BTreeMap::new(),
            data: vec![1; 100],
        },
        Codec::Zstd,
    )
    .unwrap();
    let bytes = w.finish(BTreeMap::new(), false).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let run = |args: &[&std::ffi::OsStr]| {
        Command::new(env!("CARGO_BIN_EXE_wc-lake"))
            .arg("segment-budget")
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(&[path.as_os_str(), path.as_os_str()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["artifacts"], 2);
    assert_eq!(report["payload_bytes"], 200);
    assert_eq!(report["compressed_bytes"], bytes.len() * 2);
    for args in [
        vec![],
        vec!["--unknown".as_ref()],
        vec![path.as_os_str(), "--unknown".as_ref()],
        vec![path.as_os_str(); 193],
    ] {
        assert!(!run(&args).status.success());
    }
    std::fs::write(&path, b"bad").unwrap();
    assert!(!run(&[path.as_os_str()]).status.success());
    std::fs::remove_dir_all(tmp).unwrap();
}
