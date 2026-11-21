// SPDX-License-Identifier: Apache-2.0
//! Complete paired refresh, sharing projected rows and computed grams.
use crate::core::View;
use crate::lake::{
    DataFile,
    rowgroup::{Error, index_groups, index_groups_shared},
};
use crate::puffin::{identity, rowgroup};
use std::collections::BTreeSet;

pub struct Refresh {
    pub groups: rowgroup::GroupIndex,
    pub identities: identity::IdentityIndex,
    pub indexed_files: usize,
    pub reused_files: usize,
    pub indexed_groups: usize,
    pub reused_groups: usize,
}

/// Caller authenticates producer, complete current inventory and immutable paths.
/// Both caches must describe identical complete inventories and profiles.
pub fn refresh_search(
    files: &[DataFile],
    group_binding: rowgroup::Binding,
    identity_binding: identity::Binding,
    views: &[View],
    max_value_bytes: usize,
    previous: Option<(&rowgroup::GroupIndex, &identity::IdentityIndex)>,
) -> Result<Refresh, Error> {
    if group_binding.table_uuid != identity_binding.table_uuid
        || group_binding.column != identity_binding.column
        || group_binding.column_field_id != identity_binding.column_field_id
        || group_binding.guard == identity_binding.guard
        || group_binding.guard_field_id == identity_binding.guard_field_id
    {
        return Err(Error::Invalid("paired binding mismatch"));
    }
    // Both builders validate configuration before opening any Parquet.
    let config = index_groups(&[], group_binding.clone(), views, max_value_bytes)?;
    let mut validation = identity::Builder::new(identity_binding.clone(), views, max_value_bytes)?;
    for file in files {
        validation.start_file(&file.lake_path)?;
    }
    let validation = validation.finish();
    let mut known = BTreeSet::new();
    if let Some((groups, ids)) = previous {
        if groups.binding() != &group_binding
            || ids.binding() != &identity_binding
            || groups.grams().views != config.grams().views
            || ids.views() != validation.views()
            || groups.max_value_bytes() != max_value_bytes
            || ids.max_value_bytes() != max_value_bytes
        {
            return Err(Error::Invalid("paired reuse profile mismatch"));
        }
        known = groups.files().iter().map(|f| f.path.as_str()).collect();
        if known != ids.files().iter().map(|f| f.path()).collect() {
            return Err(Error::Invalid("paired reuse inventory mismatch"));
        }
    }
    let fresh: Vec<_> = files
        .iter()
        .filter(|f| !known.contains(f.lake_path.as_str()))
        .map(|f| DataFile {
            local: f.local.clone(),
            lake_path: f.lake_path.clone(),
        })
        .collect();
    let mut builder = identity::Builder::new(identity_binding.clone(), views, max_value_bytes)?;
    let groups = index_groups_shared(
        &fresh,
        group_binding,
        views,
        max_value_bytes,
        Some((&identity_binding, &mut builder)),
    )?;
    let ids = builder.finish();
    let indexed_files = groups.files().len();
    let indexed_groups = groups.groups().len();
    let paths: Vec<_> = files.iter().map(|f| f.lake_path.clone()).collect();
    let (groups, identities) = if let Some((old_groups, old_ids)) = previous {
        (
            old_groups.assemble(&groups, &paths)?,
            old_ids.assemble(&ids, &paths)?,
        )
    } else {
        (groups, ids)
    };
    Ok(Refresh {
        reused_files: groups.files().len() - indexed_files,
        reused_groups: groups.groups().len() - indexed_groups,
        indexed_files,
        indexed_groups,
        groups,
        identities,
    })
}
