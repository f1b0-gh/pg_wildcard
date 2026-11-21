// SPDX-License-Identifier: Apache-2.0
use super::*;
fn binding() -> Binding {
    Binding {
        table_uuid: "00000000-0000-4000-8000-000000000001".into(),
        column: "message".into(),
        guard: "event_id".into(),
        column_field_id: 1,
        guard_field_id: 2,
    }
}
#[test]
fn rare_grams_keep_complete_ids_and_duplicate_guards_do_not_saturate() {
    let mut b = Builder::new(binding(), &[View::Raw], 8192).unwrap();
    b.start_file("s3://lake/a").unwrap();
    for _ in 0..50 {
        b.add_value(Some(b"powershell.exe EncodedCommand"), Some("same-id"))
            .unwrap();
    }
    b.add_value(Some(b"powershell.exe EncodedCommand"), Some("other-id"))
        .unwrap();
    b.add_value(Some(b"ordinary"), Some("negative")).unwrap();
    let idx = b.finish();
    let p = idx
        .plan(&binding(), Kind::Wildcard, "*EncodedCommand*", View::Raw)
        .unwrap();
    assert!(!p.full_scan);
    assert_eq!(
        p.files[0].values,
        Some(vec!["other-id".into(), "same-id".into()])
    );
}
#[test]
fn saturated_or_missing_postings_and_unusable_rows_widen() {
    let mut b = Builder::new(binding(), &[View::Raw], 32).unwrap();
    b.start_file("s3://lake/a").unwrap();
    for i in 0..40 {
        b.add_value(Some(b"repeated"), Some(&i.to_string()))
            .unwrap();
    }
    let p = b
        .finish()
        .plan(&binding(), Kind::Wildcard, "*repeated*", View::Raw)
        .unwrap();
    assert!(p.full_scan || p.files[0].values.is_none());
    for (raw, id) in [(vec![b'x'; 33], Some("id")), (b"target".to_vec(), None)] {
        let mut b = Builder::new(binding(), &[View::Raw], 32).unwrap();
        b.start_file("s3://lake/a").unwrap();
        b.add_value(Some(b"target"), Some("known")).unwrap();
        b.add_value(Some(&raw), id).unwrap();
        let p = b
            .finish()
            .plan(&binding(), Kind::Wildcard, "*target*", View::Raw)
            .unwrap();
        assert!(p.full_scan || p.files[0].values.is_none());
    }
}
#[test]
fn sparse_codec_roundtrip_and_every_truncation_is_rejected() {
    let mut b = Builder::new(binding(), &View::ALL, 8192).unwrap();
    b.start_file("s3://lake/a").unwrap();
    b.add_value(Some(b"PowerShell.exe"), Some("id\0'\\é"))
        .unwrap();
    let idx = b.finish();
    let bytes = idx.write(41, 7).unwrap();
    let decoded = IdentityIndex::read(&bytes).unwrap();
    assert_eq!(idx, decoded);
    for n in 0..bytes.len() {
        assert!(
            IdentityIndex::read(&bytes[..n]).is_err(),
            "accepted prefix {n}"
        );
    }
    let mut bad = binding();
    bad.guard_field_id = 3;
    assert!(
        decoded
            .plan(&bad, Kind::Wildcard, "*Power*", View::Raw)
            .is_err()
    );
}

#[test]
fn footer_binding_and_unsupported_views_never_supply_candidates() {
    use crate::puffin::puffin::{Blob, Codec, PuffinWriter, read_footer};
    let mut builder = Builder::new(binding(), &[View::Raw], 8192).unwrap();
    builder.start_file("file").unwrap();
    builder.add_value(Some(b"target"), Some("id")).unwrap();
    let index = builder.finish();
    assert!(
        index
            .plan(&binding(), Kind::Wildcard, "*target*", View::Decoded)
            .unwrap()
            .full_scan
    );
    let footer = read_footer(&index.write(1, 0).unwrap()).unwrap();
    for case in 0..5 {
        let mut meta = footer.blobs[0].clone();
        match case {
            0 => meta.fields.reverse(),
            1 => {
                meta.properties.insert("guard".into(), "other".into());
            }
            2 => {
                meta.properties
                    .insert("table-uuid".into(), "unknown".into());
            }
            3 => {
                meta.properties.insert("guard-type".into(), "int64".into());
            }
            _ => meta.blob_type = "wildcard-sparse-identity-v3".into(),
        }
        let mut writer = PuffinWriter::new();
        writer
            .add(
                Blob {
                    blob_type: meta.blob_type,
                    fields: meta.fields,
                    snapshot_id: 1,
                    sequence_number: 0,
                    properties: meta.properties,
                    data: index.encode_payload().unwrap(),
                },
                Codec::Zstd,
            )
            .unwrap();
        assert!(IdentityIndex::read(&writer.finish(BTreeMap::new(), false).unwrap()).is_err());
    }
}

#[test]
fn every_view_can_narrow_a_rare_match_without_changing_raw_evidence() {
    let mut builder = Builder::new(binding(), &View::ALL, 8192).unwrap();
    builder.start_file("file").unwrap();
    let value = br"https://EVIL.example.test/%50ayload\drop";
    builder.add_value(Some(value), Some("target")).unwrap();
    for i in 0..40 {
        builder
            .add_value(Some(b"ordinary message"), Some(&format!("other-{i}")))
            .unwrap();
    }
    let index = IdentityIndex::read(&builder.finish().write(1, 0).unwrap()).unwrap();
    for (view, pattern) in [
        (View::Raw, "*EVIL*"),
        (View::Lower, "*evil*"),
        (View::Decoded, "*payload*"),
        (View::RevDomain, "*test.example.evil*"),
        (View::WinPath, "*/drop*"),
    ] {
        let matcher = Matcher::new(Kind::Wildcard, pattern, view).unwrap();
        assert!(matcher.is_match(value));
        let plan = index
            .plan(&binding(), Kind::Wildcard, pattern, view)
            .unwrap();
        assert!(!plan.full_scan, "{view:?}");
        assert_eq!(
            plan.files[0].values,
            Some(vec!["target".into()]),
            "{view:?}"
        );
    }
}
#[test]
fn seeded_all_view_plans_never_lose_matching_guards() {
    use crate::core::verify::Matcher;
    let mut b = Builder::new(binding(), &View::ALL, 8192).unwrap();
    b.start_file("file").unwrap();
    let bases: [&[u8]; 9] = [
        b"PowerShell.exe -EncodedCommand",
        br"C:\Users\A\Temp\stage-loader.exe",
        b"https://LOGIN.example.test/%2e%2e/admin.php",
        b"a.c2-beacon.example.test",
        b"ordinary",
        b"\xff\0PowerShell",
        b"",
        b"%zz%4%",
        b"user@host:42.",
    ];
    let mut rows = vec![];
    let mut seed = 42u64;
    for i in 0..512 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let mut value = bases[seed as usize % bases.len()].to_vec();
        value.extend_from_slice(format!("/{i}").as_bytes());
        let id = format!("id-{}", i % 41);
        b.add_value(Some(&value), Some(&id)).unwrap();
        rows.push((value, id));
    }
    let idx = IdentityIndex::read(&b.finish().write(1, 0).unwrap()).unwrap();
    let patterns = [
        "*PowerShell*",
        "*EncodedCommand*",
        "*stage-loader.exe*",
        "*example*",
        "*absent*",
        "?",
        "*",
        "",
        "*%*",
    ];
    for view in View::ALL {
        for pattern in patterns {
            let m = Matcher::new(Kind::Wildcard, pattern, view).unwrap();
            let plan = idx.plan(&binding(), Kind::Wildcard, pattern, view).unwrap();
            for (value, id) in &rows {
                if m.is_match(value) {
                    assert!(
                        plan.full_scan
                            || plan.files[0].values.as_ref().is_none_or(|s| s.contains(id)),
                        "lost {pattern:?}/{view:?}"
                    );
                }
            }
        }
        for pattern in [
            "PowerShell|stage-loader",
            "Power(Shell|Point).*Encoded",
            ".*",
            "a[.]c2-beacon",
            "(?i)powershell",
        ] {
            let kind = Kind::Regex {
                case_insensitive: false,
            };
            let m = Matcher::new(kind, pattern, view).unwrap();
            let plan = idx.plan(&binding(), kind, pattern, view).unwrap();
            for (value, id) in &rows {
                if m.is_match(value) {
                    assert!(
                        plan.full_scan
                            || plan.files[0].values.as_ref().is_none_or(|s| s.contains(id))
                    );
                }
            }
        }
    }
}
#[test]
fn boolean_unknown_branches_and_global_plan_budget_are_conservative() {
    use crate::core::grams::key;
    let a = key(View::Raw, 97, 98, 99);
    let b = key(View::Raw, 100, 101, 102);
    let map = BTreeMap::from([
        (a, BTreeSet::from(["a".into(), "shared".into()])),
        (b, BTreeSet::from(["b".into(), "shared".into()])),
    ]);
    assert_eq!(
        evaluate(
            &GramQuery::and(vec![GramQuery::Gram(a), GramQuery::Gram(b)]),
            &map
        ),
        Some(BTreeSet::from(["shared".into()]))
    );
    assert!(
        evaluate(
            &GramQuery::or(vec![GramQuery::Gram(a), GramQuery::Gram(99)]),
            &map
        )
        .is_none()
    );
    let mut builder = Builder::new(binding(), &[View::Raw], 8192).unwrap();
    for f in 0..5 {
        builder.start_file(&format!("file-{f}")).unwrap();
        for i in 0..32 {
            builder
                .add_value(Some(b"target"), Some(&format!("id-{f}-{i}")))
                .unwrap();
        }
        builder
            .add_value(Some(b"ordinary"), Some("negative"))
            .unwrap();
    }
    assert!(
        builder
            .finish()
            .plan(&binding(), Kind::Wildcard, "*target*", View::Raw)
            .unwrap()
            .full_scan
    );
}
#[test]
fn builder_budget_fallback_does_not_reintroduce_partial_postings() {
    let mut b = Builder::new(binding(), &[View::Raw], 8192).unwrap();
    b.start_file("file").unwrap();
    // Exercise each remaining-global-budget check without building a million entries.
    b.retained.grams = MAX_GRAMS;
    b.add_value(Some(b"target"), Some("one")).unwrap();
    b.retained = Usage::default();
    b.add_value(Some(b"target"), Some("two")).unwrap();
    let idx = b.finish();
    assert!(idx.files()[0].unrestricted());
    for which in 0..2 {
        let mut b = Builder::new(binding(), &[View::Raw], 8192).unwrap();
        b.start_file("file").unwrap();
        if which == 0 {
            b.retained.postings = MAX_POSTINGS
        } else {
            b.retained.bytes = MAX_GUARD_BYTES
        };
        b.add_value(Some(b"target"), Some("one")).unwrap();
        assert!(b.current.as_ref().unwrap().unrestricted);
    }
}
#[test]
fn payload_refuses_versions_counts_grammar_and_trailing_bytes() {
    let mut b = Builder::new(binding(), &[View::Raw], 8192).unwrap();
    b.start_file("file").unwrap();
    b.add_value(Some(b"abc"), Some("id")).unwrap();
    let idx = b.finish();
    let data = idx.encode_payload_version(1).unwrap();
    for i in 0..data.len() {
        assert!(IdentityIndex::decode_payload(&data[..i]).is_err());
    }
    let mut bad = data.clone();
    bad[4] = 3;
    assert!(IdentityIndex::decode_payload(&bad).is_err());
    let mut bad = data.clone();
    bad[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(IdentityIndex::decode_payload(&bad).is_err());
    let header = u32::from_le_bytes(data[5..9].try_into().unwrap()) as usize;
    let view_count = 9 + header + 4;
    let files = view_count + 4 + 1;
    for offset in [view_count, files] {
        let mut bad = data.clone();
        bad[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(IdentityIndex::decode_payload(&bad).is_err());
    }
    let gram_count = files + 4 + 4 + 4 + 8 + 1;
    let gram = gram_count + 4;
    for offset in [gram_count, gram + 8, gram + 12] {
        let mut bad = data.clone();
        bad[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(IdentityIndex::decode_payload(&bad).is_err());
    }
    let mut bad = data.clone();
    bad[gram..gram + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(IdentityIndex::decode_payload(&bad).is_err());
    let mut bad = data;
    bad.push(0);
    assert!(IdentityIndex::decode_payload(&bad).is_err());
}

#[test]
fn full_coverage_guards_are_unrestricted_before_budget_charging() {
    let mut b = Builder::new(binding(), &[View::Raw], 8192).unwrap();
    for f in 0..5 {
        b.start_file(&format!("full-{f}")).unwrap();
        for i in 0..32 {
            b.add_value(Some(b"target"), Some(&format!("id-{f}-{i}")))
                .unwrap();
        }
    }
    b.start_file("singleton").unwrap();
    b.add_value(Some(b"target"), Some("single")).unwrap();
    b.start_file("useful").unwrap();
    b.add_value(Some(b"target"), Some("candidate")).unwrap();
    b.add_value(Some(b"ordinary"), Some("negative")).unwrap();
    let p = b
        .finish()
        .plan(&binding(), Kind::Wildcard, "*target*", View::Raw)
        .unwrap();
    assert!(
        !p.full_scan,
        "nonselective files must not exhaust the useful guard budget"
    );
    assert!(p.files[..6].iter().all(|f| f.values.is_none()));
    assert_eq!(p.files[6].values, Some(vec!["candidate".into()]));
}

#[test]
fn entirely_covered_files_need_no_identity_plan() {
    for count in [1, 3, 32] {
        let mut b = Builder::new(binding(), &[View::Raw], 8192).unwrap();
        b.start_file("file").unwrap();
        for i in 0..count {
            b.add_value(Some(b"target"), Some(&format!("id-{i}")))
                .unwrap();
        }
        let index = b.finish();
        for kind in [
            Kind::Wildcard,
            Kind::Regex {
                case_insensitive: false,
            },
        ] {
            let pattern = if kind == Kind::Wildcard {
                "*target*"
            } else {
                "target"
            };
            assert!(
                index
                    .plan(&binding(), kind, pattern, View::Raw)
                    .unwrap()
                    .full_scan
            );
        }
    }
}

#[test]
fn widening_preserves_empty_sets_null_rows_and_duplicate_guards() {
    let mut b = Builder::new(binding(), &[View::Raw], 8192).unwrap();
    // Both required trigrams exist, but their guard intersection is empty.
    b.start_file("empty").unwrap();
    b.add_value(Some(b"abc"), Some("a")).unwrap();
    b.add_value(Some(b"xyz"), Some("b")).unwrap();
    b.start_file("null").unwrap();
    b.add_value(Some(b"abc xyz"), Some("target")).unwrap();
    b.add_value(None, Some("null-message")).unwrap();
    b.start_file("duplicates").unwrap();
    b.add_value(Some(b"abc xyz"), Some("shared")).unwrap();
    b.add_value(Some(b"abc xyz"), Some("shared")).unwrap();
    b.add_value(Some(b"ordinary"), Some("negative")).unwrap();
    let p = b
        .finish()
        .plan(&binding(), Kind::Wildcard, "*abc*xyz*", View::Raw)
        .unwrap();
    assert!(!p.full_scan);
    assert_eq!(p.files[0].values, Some(vec![]));
    assert_eq!(p.files[1].values, Some(vec!["target".into()]));
    assert_eq!(p.files[2].values, Some(vec!["shared".into()]));
}

#[test]
fn visiting_grams_preserves_sparse_bytes_and_complete_group_input_after_fallback() {
    for force_budget in [false, true] {
        let mut old = Builder::new(binding(), &View::ALL, 32).unwrap();
        let mut shared = Builder::new(binding(), &View::ALL, 32).unwrap();
        for name in ["saturation", "fallback"] {
            old.start_file(name).unwrap();
            shared.start_file(name).unwrap();
            if force_budget {
                old.retained.grams = MAX_GRAMS;
                shared.retained.grams = MAX_GRAMS;
            }
            let mut rows: Vec<(Option<Vec<u8>>, Option<String>)> = (0..40)
                .map(|i| (Some(b"repeated target".to_vec()), Some(format!("id-{i}"))))
                .collect();
            rows.extend([
                (None, None),
                (Some(vec![b'x'; 33]), Some("long".into())),
                (Some(b"new rare gram".to_vec()), None),
                (Some(b"after fallback".to_vec()), Some("known".into())),
                (Some(b"\xff\0%41\\path".to_vec()), Some("x".repeat(257))),
                (Some(vec![]), Some("empty".into())),
            ]);
            for (raw, id) in rows {
                let mut seen = vec![];
                old.add_value(raw.as_deref(), id.as_deref()).unwrap();
                shared
                    .add_value_visiting_grams(raw.as_deref(), id.as_deref(), |g| seen.push(g))
                    .unwrap();
                let mut expected = vec![];
                if let Some(raw) = raw.as_deref().filter(|r| r.len() <= 32) {
                    for view in View::ALL {
                        // Independently form anchored windows, not the streaming iterator.
                        expected.extend(crate::core::grams::literal_grams(
                            view,
                            &view.apply(raw),
                            true,
                            true,
                        ));
                    }
                }
                seen.sort_unstable();
                seen.dedup();
                expected.sort_unstable();
                expected.dedup();
                assert_eq!(seen, expected, "lost group grams after sparse fallback");
            }
        }
        let old = old.finish();
        let shared = shared.finish();
        assert_eq!(old, shared);
        assert_eq!(old.write(42, 1).unwrap(), shared.write(42, 1).unwrap());
    }
}

#[test]
fn panicking_gram_observer_cannot_leave_partial_sparse_postings() {
    let mut b = Builder::new(binding(), &[View::Raw], 32).unwrap();
    b.start_file("file").unwrap();
    b.add_value(Some(b"abc ordinary"), Some("first")).unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            b.add_value_visiting_grams(Some(b"abc target"), Some("second"), |_| {
                panic!("observer failed")
            })
            .unwrap();
        }))
        .is_err()
    );
    assert!(
        b.finish()
            .plan(&binding(), Kind::Wildcard, "*abc*", View::Raw)
            .unwrap()
            .full_scan
    );
}
