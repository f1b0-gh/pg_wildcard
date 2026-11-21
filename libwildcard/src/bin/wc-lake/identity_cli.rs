// SPDX-License-Identifier: Apache-2.0
//! Experimental CLI; never publishes into an active manifest or registers statistics.
use crate::Args;
use libwildcard::lake::DataFile;
use libwildcard::puffin::identity::{Binding, IdentityIndex, MAX_FILE_BYTES};
use std::{
    collections::HashMap,
    io::{Read, Write},
};

pub fn run(cmd: &str, argv: &[String]) -> Result<(), String> {
    let refresh = cmd == "refresh-identities";
    let building = cmd != "plan-identities";
    let common = [
        "table-uuid",
        "column",
        "guard",
        "field-id",
        "guard-field-id",
    ];
    let allowed: &[&str] = if building {
        &[
            "views",
            "max-value-bytes",
            "snapshot-id",
            "sequence-number",
            "out",
        ]
    } else {
        &["kind", "view", "pattern", "guard-sql"]
    };
    let mut a = Args {
        pos: Vec::new(),
        opt: HashMap::new(),
    };
    let mut rest = argv.iter();
    while let Some(arg) = rest.next() {
        if let Some(key) = arg.strip_prefix("--") {
            if !common.contains(&key)
                && !allowed.contains(&key)
                && !(refresh && key == "reuse-identities")
            {
                return Err("unknown identity option".into());
            }
            let value = rest.next().ok_or("identity option requires a value")?;
            if a.opt.insert(key.into(), value.clone()).is_some() {
                return Err("duplicate identity option".into());
            }
        } else {
            a.pos.push(arg.clone());
        }
    }
    let binding = Binding {
        table_uuid: a.get("table-uuid")?.into(),
        column: a.get("column")?.into(),
        guard: a.get("guard")?.into(),
        column_field_id: a.num("field-id", -1)?,
        guard_field_id: a.num("guard-field-id", -1)?,
    };
    if building {
        let snapshot_id: i64 = a
            .get("snapshot-id")?
            .parse()
            .map_err(|_| "invalid snapshot ID")?;
        let sequence: i64 = a
            .get("sequence-number")?
            .parse()
            .map_err(|_| "invalid sequence number")?;
        if snapshot_id < 0 || sequence < 0 {
            return Err("negative snapshot/sequence".into());
        }
        let out = a.get("out")?;
        if a.pos.is_empty() && !refresh {
            return Err("no input files".into());
        }
        let views = a
            .get("views")
            .unwrap_or("raw")
            .split(',')
            .map(str::parse)
            .collect::<Result<Vec<_>, _>>()?;
        let files: Vec<_> = a
            .pos
            .iter()
            .map(|p| {
                let (local, lake_path) = p.split_once('=').unwrap_or((p, p));
                DataFile {
                    local: local.into(),
                    lake_path: lake_path.into(),
                }
            })
            .collect();
        let previous = if let Some(path) = a.opt.get("reuse-identities") {
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .map_err(|e| e.to_string())?
                .take(MAX_FILE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            Some(IdentityIndex::read(&bytes).map_err(|e| e.to_string())?)
        } else {
            None
        };
        let result = libwildcard::lake::identity::refresh_identities(
            &files,
            binding,
            &views,
            a.num("max-value-bytes", 8192)?,
            previous.as_ref(),
        )
        .map_err(|e| e.to_string())?;
        let index = result.index;
        let rows = index
            .files()
            .iter()
            .try_fold(0u64, |n, f| n.checked_add(f.rows()))
            .ok_or("row count overflow")?;
        let report = serde_json::json!({"version":1,"binding":index.binding(),
            "snapshot_id":snapshot_id,"sequence_number":sequence,
            "views":index.views().iter().map(|v|v.name()).collect::<Vec<_>>(),
            "max_value_bytes":index.max_value_bytes(),"paths":index.files().iter().map(|f|f.path()).collect::<Vec<_>>(),
            "indexed_files":result.indexed_files,"reused_files":result.reused_files});
        let bytes = index
            .write(snapshot_id, sequence)
            .map_err(|e| e.to_string())?;
        // Serialize fully before creating a fresh destination; never overwrite an index.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(out)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        eprintln!(
            "indexed {} files, {} rows, {} grams, {} unrestricted files, {} bytes (experimental)",
            index.files().len(),
            rows,
            index.files().iter().map(|f| f.grams()).sum::<usize>(),
            index.files().iter().filter(|f| f.unrestricted()).count(),
            bytes.len()
        );
        if refresh {
            println!(
                "{}",
                serde_json::to_string(&report).map_err(|e| e.to_string())?
            );
        }
    } else {
        if a.pos.len() != 1 {
            return Err("expected one identity index".into());
        }
        let sql = match a.get("guard-sql").unwrap_or("off") {
            "off" => false,
            "on" => true,
            _ => return Err("--guard-sql must be on or off".into()),
        };
        let mut bytes = Vec::new();
        std::fs::File::open(&a.pos[0])
            .map_err(|e| e.to_string())?
            .take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let index = IdentityIndex::read(&bytes).map_err(|e| e.to_string())?;
        let plan = index
            .plan(
                &binding,
                a.get("kind")?.parse()?,
                a.get("pattern")?,
                a.get("view")?.parse()?,
            )
            .map_err(|e| e.to_string())?;
        let mut value = serde_json::to_value(&plan).map_err(|e| e.to_string())?;
        if sql {
            for (i, file) in plan.files.iter().enumerate() {
                value["files"][i]["guard_sql"] = libwildcard::antalya::identities::predicate(
                    &binding.guard,
                    file.values.as_deref(),
                )
                .map_err(|e| e.to_string())?
                .into();
            }
        }
        println!(
            "{}",
            serde_json::to_string(&value).map_err(|e| e.to_string())?
        );
    }
    Ok(())
}
