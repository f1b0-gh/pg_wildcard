// SPDX-License-Identifier: Apache-2.0
//! Opt-in immutable-path refresh; the caller authenticates the current inventory
//! and old index. The final manifest is the publication marker, written last.
use super::{Args, group_binding};
use libwildcard::core::View;
use libwildcard::lake::{DataFile, rowgroup::refresh_groups};
use libwildcard::puffin::{
    IndexInfo,
    rowgroup::{GroupIndex, MAX_FILE_BYTES},
    write_index,
};
use std::{fs, io::Write, path::Path};

pub(super) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

pub(super) fn run(a: &Args) -> Result<(), String> {
    const OPTIONS: &[&str] = &[
        "out-dir",
        "reuse-groups",
        "table-uuid",
        "column",
        "field-id",
        "guard",
        "guard-field-id",
        "guard-type",
        "views",
        "max-value-bytes",
        "snapshot-id",
        "sequence-number",
    ];
    if a.opt.keys().any(|k| !OPTIONS.contains(&k.as_str())) {
        return Err("unknown refresh-groups option".into());
    }
    let out = Path::new(a.get("out-dir")?);
    if out.try_exists().map_err(|e| e.to_string())? {
        return Err("refresh output directory already exists".into());
    }
    let info = IndexInfo {
        column: a.get("column")?.into(),
        field_id: a.num("field-id", -1)?,
        snapshot_id: a.num("snapshot-id", -1)?,
        sequence_number: a.num("sequence-number", -1)?,
        max_value_bytes: a.num("max-value-bytes", 8192)?,
    };
    if info.snapshot_id <= 0 || info.sequence_number < 0 {
        return Err("positive snapshot-id and nonnegative sequence-number required".into());
    }
    let views = a
        .get("views")
        .unwrap_or("raw")
        .split(',')
        .map(str::parse)
        .collect::<Result<Vec<View>, _>>()?;
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
    let previous = if let Some(path) = a.opt.get("reuse-groups") {
        let bytes = crate::read_index_file(path)?;
        Some(GroupIndex::read(&bytes).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let result = refresh_groups(
        &files,
        group_binding(a)?,
        &views,
        info.max_value_bytes,
        previous.as_ref(),
    )
    .map_err(|e| e.to_string())?;
    let groups = result
        .index
        .write(info.snapshot_id, info.sequence_number)
        .map_err(|e| e.to_string())?;
    let file_index = result.index.file_index().map_err(|e| e.to_string())?;
    let file_bytes = write_index(&file_index, &info).map_err(|e| e.to_string())?;
    if file_bytes.len() > MAX_FILE_BYTES {
        return Err("derived file sidecar exceeds envelope budget".into());
    }
    let manifest = serde_json::json!({"version":1,"binding":result.index.binding(),"snapshot_id":info.snapshot_id,"sequence_number":info.sequence_number,
        "views":result.index.grams().views.iter().map(|v|v.name()).collect::<Vec<_>>(),"max_value_bytes":info.max_value_bytes,
        "paths":result.index.files().iter().map(|f|&f.path).collect::<Vec<_>>(),"files":"files.puffin","groups":"groups.puffin","file_count":file_index.files.len(),"group_count":result.index.groups().len(),
        "indexed_files":result.indexed_files,"reused_files":result.reused_files,"indexed_groups":result.indexed_groups,"reused_groups":result.reused_groups,
        "files_bytes":file_bytes.len(),"groups_bytes":groups.len()});
    let json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    // Reserve a fresh generation. A failed write leaves no ready manifest;
    // callers must not consume a directory merely because it exists.
    fs::create_dir(out).map_err(|e| e.to_string())?;
    write_new(&out.join("groups.puffin"), &groups)?;
    write_new(&out.join("files.puffin"), &file_bytes)?;
    write_new(&out.join("manifest.pending.json"), &json)?;
    fs::rename(out.join("manifest.pending.json"), out.join("manifest.json"))
        .map_err(|e| e.to_string())?;
    eprintln!(
        "{}",
        serde_json::to_string(&manifest).map_err(|e| e.to_string())?
    );
    Ok(())
}
