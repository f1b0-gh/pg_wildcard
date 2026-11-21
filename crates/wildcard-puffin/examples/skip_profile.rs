// SPDX-License-Identifier: Apache-2.0
//! Warm in-memory candidate planning; no engine/network/Elastic latency claim.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    hint::black_box,
    io::{self, Write},
    time::Instant,
};
use wildcard_core::{
    View, grams,
    verify::{Kind, Matcher},
};
use wildcard_puffin::ngram::{FileEntry, NgramError, NgramIndex};

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Pattern(#[from] NgramError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("candidate mismatch")]
    Mismatch,
}

fn legacy<'a>(
    idx: &'a NgramIndex,
    kind: Kind,
    pattern: &str,
) -> Result<Vec<&'a FileEntry>, NgramError> {
    let matcher = Matcher::new(kind, pattern, View::Raw)?;
    let mut lookup = |g| {
        idx.grams
            .binary_search_by_key(&g, |x| x.0)
            .map_or_else(|_| Vec::new(), |i| idx.grams[i].1.clone())
    };
    let mut ids = matcher.plan.candidates(idx.files.len() as u32, &mut lookup);
    ids.extend(
        idx.files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.has_unindexed)
            .map(|(i, _)| i as u32),
    );
    ids.sort_unstable();
    ids.dedup();
    Ok(ids.into_iter().map(|i| &idx.files[i as usize]).collect())
}

fn run() -> Result<(), Error> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    writeln!(
        out,
        "units,case,round,variant,iterations,elapsed_ns,candidates"
    )?;
    for n in [32u32, 128, 4096] {
        for case in [
            "absent",
            "rare",
            "broad",
            "dense_overlap",
            "short",
            "nested",
        ] {
            let mut keys = grams::literal_grams(View::Raw, b"abcdefghijklmnopqr", false, false);
            keys.sort_unstable();
            keys.dedup();
            let last = keys.last().copied();
            let idx = NgramIndex {
                views: vec![View::Raw],
                files: (0..n)
                    .map(|i| FileEntry {
                        path: format!("unit-{i}"),
                        record_count: 8192,
                        has_unindexed: i == n - 1,
                    })
                    .collect(),
                grams: keys
                    .into_iter()
                    .filter(|g| case != "absent" || Some(*g) != last)
                    .enumerate()
                    .map(|(ordinal, g)| {
                        (
                            g,
                            if case == "rare" && Some(g) == last {
                                vec![n / 2]
                            } else if case == "dense_overlap" {
                                // Different near-full lists: equality shortcut
                                // cannot replace the real intersection work.
                                (0..n)
                                    .filter(|id| *id != (ordinal as u32 * 191) % n)
                                    .collect()
                            } else {
                                (0..n).collect()
                            },
                        )
                    })
                    .collect(),
            };
            let kind = if case == "nested" {
                Kind::Regex {
                    case_insensitive: false,
                }
            } else {
                Kind::Wildcard
            };
            let pattern = if case == "nested" {
                "(abc|def).*ghi"
            } else if case == "short" {
                "*abc*"
            } else {
                "*abcdefghijklmnopqr*"
            };
            let expected = legacy(&idx, kind, pattern)?;
            let actual = idx.candidates(kind, pattern, View::Raw)?;
            if expected != actual.files {
                return Err(Error::Mismatch);
            }
            for round in 0..12 {
                for slot in 0..2 {
                    let old = (round + slot) % 2 == 0;
                    let start = Instant::now();
                    for _ in 0..256 {
                        let files = if old {
                            legacy(black_box(&idx), kind, black_box(pattern))?
                        } else {
                            idx.candidates(kind, black_box(pattern), View::Raw)?.files
                        };
                        black_box(files);
                    }
                    writeln!(
                        out,
                        "{n},{case},{round},{},{},{},{}",
                        if old { "legacy" } else { "current" },
                        256,
                        start.elapsed().as_nanos(),
                        expected.len()
                    )?;
                }
            }
        }
    }
    out.flush()?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        let _ = writeln!(io::stderr().lock(), "{error}");
        std::process::exit(1);
    }
}
