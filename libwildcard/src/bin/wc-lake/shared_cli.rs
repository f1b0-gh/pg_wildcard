// SPDX-License-Identifier: Apache-2.0
//! Paired complete refresh; a fresh manifest is published only after all sidecars.
use crate::{Args, group_binding, refresh::write_new};
use libwildcard::core::View;
use libwildcard::lake::{DataFile, shared::refresh_search};
use libwildcard::puffin::{
    IndexInfo,
    identity::{Binding, IdentityIndex},
    puffin::read_footer,
    rowgroup::{GroupIndex, MAX_FILE_BYTES},
    write_index,
};
use std::{collections::HashMap, fs, io::Read, path::Path};

fn cached(path: &str) -> Result<(Vec<u8>, (i64, i64)), String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err("reuse sidecar exceeds byte budget".into());
    }
    let footer = read_footer(&bytes).map_err(|e| e.to_string())?;
    if footer.blobs.len() != 1 {
        return Err("reuse sidecar requires one own blob".into());
    }
    let meta = &footer.blobs[0];
    if meta.snapshot_id <= 0 || meta.sequence_number < 0 {
        return Err("invalid reuse snapshot/sequence".into());
    }
    let stamp = (meta.snapshot_id, meta.sequence_number);
    Ok((bytes, stamp))
}

pub(super) fn run(argv: &[String]) -> Result<(), String> {
    const OPTIONS: &[&str] = &[
        "out-dir",
        "reuse-groups",
        "reuse-identities",
        "table-uuid",
        "column",
        "field-id",
        "guard",
        "guard-field-id",
        "guard-type",
        "identity-guard",
        "identity-guard-field-id",
        "views",
        "max-value-bytes",
        "snapshot-id",
        "sequence-number",
    ];
    let mut a = Args {
        pos: Vec::new(),
        opt: HashMap::new(),
    };
    let mut rest = argv.iter();
    while let Some(arg) = rest.next() {
        if let Some(key) = arg.strip_prefix("--") {
            if !OPTIONS.contains(&key) {
                return Err("unknown refresh-search option".into());
            }
            let value = rest
                .next()
                .filter(|v| !v.starts_with("--"))
                .ok_or("refresh-search option requires a value")?;
            if a.opt.insert(key.into(), value.clone()).is_some() {
                return Err("duplicate refresh-search option".into());
            }
        } else {
            a.pos.push(arg.clone());
        }
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
    let previous = match (a.opt.get("reuse-groups"), a.opt.get("reuse-identities")) {
        (None, None) => None,
        (Some(g), Some(i)) => {
            let (g, gs) = cached(g)?;
            let (i, is) = cached(i)?;
            if gs != is {
                return Err("paired reuse snapshot/sequence mismatch".into());
            }
            Some((
                GroupIndex::read(&g).map_err(|e| e.to_string())?,
                IdentityIndex::read(&i).map_err(|e| e.to_string())?,
            ))
        }
        _ => return Err("both reuse-groups and reuse-identities are required".into()),
    };
    let result = refresh_search(
        &files,
        group_binding(&a)?,
        Binding {
            table_uuid: a.get("table-uuid")?.into(),
            column: info.column.clone(),
            column_field_id: info.field_id,
            guard: a.get("identity-guard")?.into(),
            guard_field_id: a.num("identity-guard-field-id", -1)?,
        },
        &views,
        info.max_value_bytes,
        previous.as_ref().map(|(g, i)| (g, i)),
    )
    .map_err(|e| e.to_string())?;
    let groups = result
        .groups
        .write(info.snapshot_id, info.sequence_number)
        .map_err(|e| e.to_string())?;
    let identities = result
        .identities
        .write(info.snapshot_id, info.sequence_number)
        .map_err(|e| e.to_string())?;
    let index = result.groups.file_index().map_err(|e| e.to_string())?;
    let files = write_index(&index, &info).map_err(|e| e.to_string())?;
    if files.len() > MAX_FILE_BYTES {
        return Err("derived file sidecar exceeds envelope budget".into());
    }
    let manifest = serde_json::json!({
        "version":1, "binding":result.groups.binding(), "identity_binding":result.identities.binding(),
        "snapshot_id":info.snapshot_id, "sequence_number":info.sequence_number,
        "views":result.groups.grams().views.iter().map(|v|v.name()).collect::<Vec<_>>(), "max_value_bytes":info.max_value_bytes,
        "paths":result.groups.files().iter().map(|f|&f.path).collect::<Vec<_>>(),
        "files":"files.puffin", "groups":"groups.puffin", "identities":"identities.puffin",
        "file_count":index.files.len(), "group_count":result.groups.groups().len(),
        "indexed_files":result.indexed_files, "reused_files":result.reused_files,
        "identity_indexed_files":result.indexed_files, "identity_reused_files":result.reused_files,
        "indexed_groups":result.indexed_groups, "reused_groups":result.reused_groups,
        "files_bytes":files.len(), "groups_bytes":groups.len(), "identities_bytes":identities.len()
    });
    let json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    if json.len() > 128 * 1024 {
        return Err("refresh manifest exceeds byte budget".into());
    }
    fs::create_dir(out).map_err(|e| e.to_string())?;
    write_new(&out.join("files.puffin"), &files)?;
    write_new(&out.join("groups.puffin"), &groups)?;
    write_new(&out.join("identities.puffin"), &identities)?;
    write_new(&out.join("manifest.pending.json"), &json)?;
    fs::rename(out.join("manifest.pending.json"), out.join("manifest.json"))
        .map_err(|e| e.to_string())?;
    eprintln!(
        "{}",
        serde_json::to_string(&manifest).map_err(|e| e.to_string())?
    );
    Ok(())
}
