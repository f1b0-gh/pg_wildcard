// SPDX-License-Identifier: Apache-2.0
//! `wc-lake`: build and use Puffin n-gram indexes for Iceberg tables.
//!
//! ```text
//! wc-lake index   --column url --views raw,decoded,revdomain --snapshot-id N [--sequence-number N]
//!                 [--field-id N] [--max-value-bytes 8192] --out idx.puffin LOCAL=LAKE_PATH...
//! wc-lake index-groups --table-uuid UUID --column message --field-id N --guard id --guard-field-id N --out idx.puffin LOCAL=LAKE_PATH...
//! wc-lake plan-groups idx.puffin --table-uuid UUID --column message --field-id N --guard id --guard-field-id N --kind K --view V --pattern P
//! wc-lake refresh-groups --out-dir NEW_DIRECTORY [--reuse-groups OLD.puffin] <index-groups options> LOCAL=IMMUTABLE_LAKE_PATH...
//! wc-lake refresh-search --out-dir NEW_DIRECTORY [--reuse-groups OLD --reuse-identities OLD] <index-groups options> --identity-guard event_id --identity-guard-field-id N LOCAL=IMMUTABLE_LAKE_PATH...
//! wc-lake refresh-identities [--reuse-identities OLD.puffin] <index-identities options> LOCAL=IMMUTABLE_LAKE_PATH...
//! wc-lake index-identities --table-uuid UUID --column message --field-id N --guard event_id --guard-field-id N --snapshot-id N --sequence-number N --out NEW.puffin LOCAL=LAKE_PATH...
//! wc-lake plan-identities idx.puffin --table-uuid UUID --column message --field-id N --guard event_id --guard-field-id N --kind K --view V --pattern P [--guard-sql on]
//! wc-lake segment-budget idx.puffin...
//! wc-lake inspect idx.puffin
//! wc-lake prune   idx.puffin --kind wildcard|like|regex|iregex --view decoded --pattern P
//! wc-lake sql     idx.puffin --table db.http --select 'ts, url' --kind K --view V --pattern P [--where EXPR] [--prune off] [--known-table wc_lake_known] [--large-inventory on]
//! wc-lake export-pg idx.puffin --name http_url --out-dir DIR [--known-table wc_lake_known] [--large-inventory on]
//! wc-lake stats-json idx.puffin --statistics-path s3://.../wc-<snapshot>.puffin
//! ```

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

mod identity_cli;
mod refresh;
mod segment_cli;
mod shared_cli;

use std::collections::HashMap;
use std::process::ExitCode;

use libwildcard::antalya::ViewColumns;
use libwildcard::core::View;
use libwildcard::core::verify::Kind;
use libwildcard::lake::{DataFile, export_pg, index_parquet};
use libwildcard::puffin::{IndexInfo, read_first_index, set_statistics_update, write_index};

fn usage() -> ExitCode {
    eprintln!(
        "{}",
        include_str!("main.rs")
            .lines()
            .skip(3)
            .take(16)
            .map(|l| l.trim_start_matches("//! "))
            .collect::<Vec<_>>()
            .join("\n")
    );
    ExitCode::from(2)
}

struct Args {
    pos: Vec<String>,
    opt: HashMap<String, String>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let (mut pos, mut opt) = (Vec::new(), HashMap::new());
    let mut i = 0;
    while i < args.len() {
        if let Some(k) = args[i].strip_prefix("--") {
            let v = args.get(i + 1).ok_or(format!("--{k} needs a value"))?;
            opt.insert(k.to_string(), v.clone());
            i += 2;
        } else {
            pos.push(args[i].clone());
            i += 1;
        }
    }
    Ok(Args { pos, opt })
}

impl Args {
    fn get(&self, k: &str) -> Result<&str, String> {
        self.opt
            .get(k)
            .map(String::as_str)
            .ok_or(format!("missing --{k}"))
    }
    fn num<T: std::str::FromStr>(&self, k: &str, default: T) -> Result<T, String> {
        self.opt.get(k).map_or(Ok(default), |v| {
            v.parse().map_err(|_| format!("--{k}: not a number"))
        })
    }
}

fn load(
    path: &str,
    large_inventory: Option<&String>,
) -> Result<
    (
        libwildcard::puffin::puffin::BlobMetadata,
        libwildcard::puffin::ngram::NgramIndex,
    ),
    String,
> {
    let bytes = read_index_file(path)?;
    let read = match large_inventory.map(String::as_str) {
        None | Some("off") => read_first_index(&bytes),
        Some("on") => libwildcard::puffin::read_first_index_large_inventory(&bytes),
        Some(_) => return Err("--large-inventory must be on or off".into()),
    };
    read.map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{path}: no wildcard-ngram-v1 blob"))
}

fn read_index_file(path: &str) -> Result<Vec<u8>, String> {
    read_file_limited(path, 16 << 20)
}

fn read_file_limited(path: &str, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > limit as u64 {
        return Err(format!("Puffin file exceeds {limit} byte budget"));
    }
    let mut out = Vec::new();
    let mut block = [0; 8192];
    loop {
        let take = block.len().min(limit - out.len() + 1);
        let n = file.read(&mut block[..take]).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(out);
        }
        if n > limit - out.len() {
            return Err(format!("Puffin file exceeds {limit} byte budget"));
        }
        if out.len() + n > out.capacity() {
            let capacity = out
                .capacity()
                .saturating_mul(2)
                .max(out.len() + n)
                .min(limit);
            out.try_reserve_exact(capacity - out.len())
                .map_err(|_| "Puffin input allocation failed")?;
        }
        out.extend_from_slice(&block[..n]);
    }
}

fn group_binding(a: &Args) -> Result<libwildcard::puffin::rowgroup::Binding, String> {
    Ok(libwildcard::puffin::rowgroup::Binding {
        table_uuid: a.get("table-uuid")?.into(),
        column: a.get("column")?.into(),
        guard: a.get("guard")?.into(),
        column_field_id: a.num("field-id", -1)?,
        guard_field_id: a.num("guard-field-id", -1)?,
        guard_type: a.get("guard-type").unwrap_or("int64").parse()?,
    })
}

fn run(cmd: &str, a: &Args) -> Result<(), String> {
    match cmd {
        "refresh-groups" => refresh::run(a)?,
        "index" | "index-groups" => {
            let views: Vec<View> = a
                .get("views")
                .unwrap_or("raw")
                .split(',')
                .map(|v| v.parse())
                .collect::<Result<_, _>>()?;
            let files: Vec<DataFile> = a
                .pos
                .iter()
                .map(|p| match p.split_once('=') {
                    Some((l, r)) => DataFile {
                        local: l.into(),
                        lake_path: r.into(),
                    },
                    None => DataFile {
                        local: p.clone(),
                        lake_path: p.clone(),
                    },
                })
                .collect();
            if files.is_empty() {
                return Err("no input files".into());
            }
            let info = IndexInfo {
                column: a.get("column")?.into(),
                field_id: a.num("field-id", -1)?,
                snapshot_id: a.num("snapshot-id", -1)?,
                sequence_number: a.num("sequence-number", -1)?,
                max_value_bytes: a.num("max-value-bytes", 8192)?,
            };
            if cmd == "index-groups" {
                let idx = libwildcard::lake::rowgroup::index_groups(
                    &files,
                    group_binding(a)?,
                    &views,
                    info.max_value_bytes,
                )
                .map_err(|e| e.to_string())?;
                let out = idx
                    .write(info.snapshot_id, info.sequence_number)
                    .map_err(|e| e.to_string())?;
                std::fs::write(a.get("out")?, &out).map_err(|e| e.to_string())?;
                eprintln!(
                    "indexed {} files, {} groups, {} grams, {} bytes (experimental)",
                    idx.files().len(),
                    idx.groups().len(),
                    idx.grams().grams.len(),
                    out.len()
                );
                return Ok(());
            }
            let idx = index_parquet(&files, &info.column, &views, info.max_value_bytes)
                .map_err(|e| e.to_string())?;
            let out = write_index(&idx, &info).map_err(|e| e.to_string())?;
            std::fs::write(a.get("out")?, &out).map_err(|e| e.to_string())?;
            eprintln!(
                "indexed {} files, {} grams, {} bytes",
                idx.files.len(),
                idx.grams.len(),
                out.len()
            );
        }
        "plan-groups" => {
            use libwildcard::puffin::rowgroup::{GroupIndex, PlanBudget};
            let path = a.pos.first().ok_or("missing file")?;
            let bytes = read_index_file(path)?;
            let idx = GroupIndex::read(&bytes).map_err(|e| e.to_string())?;
            let plan = idx
                .plan(
                    &group_binding(a)?,
                    a.get("kind")?.parse()?,
                    a.get("pattern")?,
                    a.get("view")?.parse()?,
                    PlanBudget::default(),
                )
                .map_err(|e| e.to_string())?;
            let mut value = serde_json::to_value(&plan).map_err(|e| e.to_string())?;
            match a.get("range-sql").unwrap_or("off") {
                "off" => {}
                "on" => {
                    let kind = match idx.binding().guard_type {
                        libwildcard::puffin::rowgroup::GuardType::Int64 => {
                            libwildcard::antalya::ranges::RangeType::Int64
                        }
                        libwildcard::puffin::rowgroup::GuardType::TimestampUsUtc => {
                            libwildcard::antalya::ranges::RangeType::TimestampUsUtc
                        }
                    };
                    for (i, file) in plan.files.iter().enumerate() {
                        let sql = match &file.ranges {
                            None => "1".into(),
                            Some(ranges) => libwildcard::antalya::ranges::predicate(
                                &idx.binding().guard,
                                ranges,
                                kind,
                            )
                            .map_err(|e| e.to_string())?,
                        };
                        value["files"][i]["range_sql"] = serde_json::Value::String(sql);
                    }
                }
                _ => return Err("--range-sql must be on or off".into()),
            }
            println!(
                "{}",
                serde_json::to_string(&value).map_err(|e| e.to_string())?
            );
        }
        "inspect" => {
            let (meta, idx) = load(
                a.pos.first().ok_or("missing file")?,
                a.opt.get("large-inventory"),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?
            );
            for f in &idx.files {
                println!(
                    "{}\trecords={}\tunindexed={}",
                    f.path, f.record_count, f.has_unindexed
                );
            }
        }
        "prune" | "sql" => {
            let (meta, idx) = load(
                a.pos.first().ok_or("missing file")?,
                a.opt.get("large-inventory"),
            )?;
            let kind: Kind = a.get("kind")?.parse()?;
            let view: View = a.get("view")?.parse()?;
            let pattern = a.get("pattern")?;
            let p = idx
                .candidates(kind, pattern, view)
                .map_err(|e| e.to_string())?;
            let files: Vec<String> = p.files.iter().map(|f| f.path.clone()).collect();
            if cmd == "prune" {
                eprintln!(
                    "{} of {} files (index used: {})",
                    files.len(),
                    idx.files.len(),
                    p.used_index
                );
                for f in &files {
                    println!("{f}");
                }
            } else {
                let column = meta
                    .properties
                    .get("column")
                    .cloned()
                    .unwrap_or_else(|| "value".into());
                let cols = ViewColumns::new(&column).map_err(|e| e.to_string())?;
                let pred = libwildcard::antalya::column_predicate(&cols, kind, pattern, view)
                    .map_err(|e| e.to_string())?;
                let no_prune = a.opt.get("prune").is_some_and(|v| v == "off");
                let query = if p.used_index && !no_prune {
                    let indexed = idx
                        .files
                        .iter()
                        .map(|file| file.path.clone())
                        .collect::<Vec<_>>();
                    let (sql, strategy) = libwildcard::antalya::lake_query_with_inventory(
                        a.get("table")?,
                        a.get("select").unwrap_or("*"),
                        &pred,
                        &indexed,
                        &files,
                        a.opt.get("where").map(String::as_str),
                        a.opt.get("known-table").map(String::as_str),
                    )
                    .map_err(|e| e.to_string())?;
                    eprintln!("lake path guard: {}", strategy.description());
                    sql
                } else {
                    libwildcard::antalya::lake_query(
                        a.get("table")?,
                        a.get("select").unwrap_or("*"),
                        &pred,
                        None,
                        a.opt.get("where").map(String::as_str),
                    )
                    .map_err(|e| e.to_string())?
                    .ok_or("missing full-scan query")?
                };
                println!("{query}");
            }
        }
        "export-pg" => {
            let (meta, idx) = load(
                a.pos.first().ok_or("missing file")?,
                a.opt.get("large-inventory"),
            )?;
            let column = meta.properties.get("column").cloned().unwrap_or_default();
            export_pg(
                &idx,
                &column,
                meta.snapshot_id,
                a.get("name")?,
                std::path::Path::new(a.get("out-dir")?),
            )
            .map_err(|e| e.to_string())?;
            libwildcard::lake::export_known_inventory(
                &idx,
                a.opt
                    .get("known-table")
                    .map(String::as_str)
                    .unwrap_or("wc_lake_known"),
                std::path::Path::new(a.get("out-dir")?),
            )
            .map_err(|e| e.to_string())?;
        }
        "stats-json" => {
            let path = a.pos.first().ok_or("missing file")?;
            let bytes = read_index_file(path)?;
            let v = set_statistics_update(a.get("statistics-path")?, &bytes)
                .map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?
            );
        }
        _ => return Err(format!("unknown command '{cmd}'")),
    }
    Ok(())
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = argv.split_first() else {
        return usage();
    };
    let result = if cmd == "segment-budget" {
        segment_cli::run(rest)
    } else if cmd == "refresh-search" {
        shared_cli::run(rest)
    } else if matches!(
        cmd.as_str(),
        "index-identities" | "plan-identities" | "refresh-identities"
    ) {
        identity_cli::run(cmd, rest)
    } else {
        parse(rest).and_then(|a| run(cmd, &a))
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wc-lake: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn input_limit_rejects_sparse_files_and_accepts_exact_boundary() {
        let path = std::env::temp_dir().join(format!("wc-cli-input-{}", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len((16 << 20) + 1).unwrap();
        let error = read_index_file(path.to_str().unwrap()).unwrap_err();
        assert!(error.contains("byte budget"));
        file.set_len(8193).unwrap();
        let bytes = read_file_limited(path.to_str().unwrap(), 8193).unwrap();
        assert_eq!(bytes, vec![0; 8193]);
        assert_eq!(bytes.capacity(), 8193);
        assert!(read_file_limited(path.to_str().unwrap(), 8192).is_err());
        file.set_len(0).unwrap();
        assert!(
            read_file_limited(path.to_str().unwrap(), 0)
                .unwrap()
                .is_empty()
        );
        std::fs::remove_file(path).unwrap();
    }
}
