//! Strict and best-effort consumption of a single preparation report.

use std::{fs, path::PathBuf};

use nominal::{FsError, PlanError, RejectionReason, Renamer};

#[test]
fn missing_sources_are_rejected_without_creating_destination_parents() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let p = |name: &str| root.join(name);
    fs::write(p("a"), "A").unwrap();
    let (plan, rejections) = Renamer::from_iter([
        (p("a"), p("b")),
        (p("b"), p("new/c")),
        (p("absent/source"), p("other/target")),
        (p("noop"), p("noop")),
    ])
    .prepare()
    .into_parts();
    assert_eq!(rejections.len(), 2);
    for rejection in &rejections {
        assert!(matches!(&rejection.reason,
            RejectionReason::Filesystem(FsError::Inspect { source_path, target_path, source })
                if source.kind() == std::io::ErrorKind::NotFound
                    && source_path == &rejection.renames[0].source
                    && target_path == &rejection.renames[0].target));
    }
    // Even if a rejected source later appears, it is no longer executable.
    fs::create_dir(p("absent")).unwrap();
    fs::write(p("absent/source"), "late").unwrap();
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("b")).unwrap(), "A");
    assert_eq!(fs::read_to_string(p("absent/source")).unwrap(), "late");
    assert!(!p("new").exists());
    assert!(!p("other").exists());
}

#[test]
fn recheck_inspects_missing_sources_even_when_their_target_will_be_vacated() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name: &str| dir.path().join(name);
    for name in ["a", "b", "c"] {
        fs::write(p(name), name).unwrap();
    }
    let mut plan = Renamer::from_iter([(p("a"), p("b")), (p("b"), p("c")), (p("c"), p("d"))])
        .prepare()
        .into_plan()
        .unwrap();
    fs::remove_file(p("b")).unwrap();
    let rejections = plan.reject_conflicts();
    assert_eq!(rejections.len(), 1);
    assert_eq!(rejections[0].renames[0].source, p("b"));
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("b")).unwrap(), "a");
    assert_eq!(fs::read_to_string(p("d")).unwrap(), "c");
}

#[test]
fn duplicate_groups_and_cycles_preserve_independent_chains() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    for name in ["a", "b", "c", "d", "e", "f", "g"] {
        fs::write(p(name), name).unwrap();
    }
    let pairs = [
        (p("a"), p("same")),
        (p("b"), p("same")),
        (p("c"), p("d")),
        (p("d"), p("c")),
        (p("e"), p("f")),
        (p("f"), p("new")),
        (p("g"), p("a")), // a is no longer vacated: this must be rejected too.
    ];
    let report = Renamer::from_iter(pairs.clone()).prepare();
    assert_eq!(
        report.rejections().iter().flat_map(|r| &r.renames).count(),
        5
    );
    assert_eq!(report.rejections().len(), 3);
    assert!(matches!(
        &report.rejections()[0].reason,
        RejectionReason::Plan(PlanError::DuplicateTarget { .. })
    ));
    let error = report.into_plan().unwrap_err();
    assert_eq!(
        error.to_string(),
        "5 rename operations rejected during preparation"
    );
    let mut sources: Vec<_> = error
        .rejections
        .iter()
        .flat_map(|r| &r.renames)
        .map(|r| r.source.clone())
        .collect();
    sources.sort();
    assert_eq!(sources, [p("a"), p("b"), p("c"), p("d"), p("g")]);
    // Strict conversion performs no part of the batch.
    for name in ["a", "b", "c", "d", "e", "f", "g"] {
        assert_eq!(fs::read_to_string(p(name)).unwrap(), name);
    }
    let (plan, rejections) = Renamer::from_iter(pairs).prepare().into_parts();
    assert_eq!(rejections.iter().map(|r| r.renames.len()).sum::<usize>(), 5);
    let completed: Vec<_> = plan.apply_iter().map(Result::unwrap).collect();
    assert_eq!(completed.len(), 2);
    assert_eq!(completed[0].source, p("f"));
    assert_eq!(fs::read_to_string(p("f")).unwrap(), "e");
    assert_eq!(fs::read_to_string(p("new")).unwrap(), "f");
}

#[test]
fn intersecting_duplicate_groups_have_no_arbitrary_survivor_or_double_count() {
    for reverse in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let p = |name| dir.path().join(name);
        for name in ["a", "b", "c"] {
            fs::write(p(name), name).unwrap();
        }
        let mut pairs = vec![
            (p("a"), p("x")),
            (p("a"), p("y")),
            (p("b"), p("y")),
            (p("c"), p("z")),
        ];
        if reverse {
            pairs.reverse();
        }
        let report = Renamer::from_iter(pairs).prepare();
        assert_eq!(
            report.rejections().iter().flat_map(|r| &r.renames).count(),
            3
        );
        let (plan, _) = report.into_parts();
        assert_eq!(plan.len(), 1);
        plan.apply().unwrap();
        assert_eq!(fs::read_to_string(p("z")).unwrap(), "c");
        assert!(p("a").exists());
        assert!(p("b").exists());
        assert!(!p("x").exists());
        assert!(!p("y").exists());
    }
}

#[test]
fn large_intersecting_duplicate_groups_reject_every_contender() {
    for reverse in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let p = |name: &str| dir.path().join(name);
        fs::write(p("shared"), "shared").unwrap();
        fs::write(p("independent"), "independent").unwrap();
        let mut pairs = Vec::new();
        for index in 0..8 {
            let source = p(&format!("source-{index}"));
            fs::write(&source, "contender").unwrap();
            pairs.push((p("shared"), p(&format!("target-{index}"))));
            pairs.push((source, p("target-0")));
        }
        let mut expected = pairs.clone();
        expected.sort();
        pairs.push((p("independent"), p("done")));
        if reverse {
            pairs.reverse();
        }
        let (plan, rejections) = Renamer::from_iter(pairs).prepare().into_parts();
        assert_eq!(rejections.len(), 2);
        let mut rejected: Vec<_> = rejections
            .iter()
            .flat_map(|r| &r.renames)
            .map(|r| (r.source.clone(), r.target.clone()))
            .collect();
        rejected.sort();
        assert_eq!(rejected, expected);
        assert_eq!(plan.len(), 1);
        plan.apply().unwrap();
        assert_eq!(fs::read_to_string(p("done")).unwrap(), "independent");
        assert_eq!(fs::read_to_string(p("shared")).unwrap(), "shared");
        for index in 0..8 {
            assert_eq!(
                fs::read_to_string(p(&format!("source-{index}"))).unwrap(),
                "contender"
            );
            assert!(!p(&format!("target-{index}")).exists());
        }
    }
}

#[test]
fn overlaps_reject_all_affected_operations_but_keep_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir_all(p("folder/sub")).unwrap();
    fs::write(p("folder/child"), "child").unwrap();
    fs::write(p("folder/sibling"), "sibling").unwrap();
    fs::write(p("folder/sub/grandchild"), "grandchild").unwrap();
    fs::write(p("a"), "a").unwrap();
    let report = Renamer::from_iter([
        (p("folder"), p("moved")),
        (p("folder/child"), p("child")),
        // Revisit the same ancestor, both directly and through a subdirectory.
        (p("folder/sibling"), p("sibling")),
        (p("folder/sub/grandchild"), p("grandchild")),
        (p("a"), p("b")),
    ])
    .prepare();
    assert_eq!(
        report.rejections().iter().flat_map(|r| &r.renames).count(),
        4
    );
    assert!(matches!(
        &report.rejections()[0].reason,
        RejectionReason::Plan(PlanError::OverlappingPaths { .. })
    ));
    report.into_parts().0.apply().unwrap();
    assert_eq!(fs::read_to_string(p("folder/child")).unwrap(), "child");
    assert_eq!(fs::read_to_string(p("folder/sibling")).unwrap(), "sibling");
    assert_eq!(
        fs::read_to_string(p("folder/sub/grandchild")).unwrap(),
        "grandchild"
    );
    assert_eq!(fs::read_to_string(p("b")).unwrap(), "a");
}

#[test]
fn duplicate_ancestor_groups_reject_every_descendant_without_double_counting() {
    for reverse in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let p = |name: &str| dir.path().join(name);
        fs::create_dir(p("folder")).unwrap();
        fs::create_dir(p("alias-parent")).unwrap();
        let mut pairs = Vec::new();
        for index in 0..64 {
            let child = p(&format!("folder/file-{index}"));
            fs::write(&child, "child").unwrap();
            // Alternate resolved spellings of the same ancestor.
            let ancestor = if index % 2 == 0 {
                p("folder")
            } else {
                p("alias-parent/../folder")
            };
            pairs.push((ancestor, p(&format!("moved-{index}"))));
            pairs.push((child, p(&format!("file-{index}"))));
        }
        if reverse {
            pairs.reverse();
        }
        fs::write(p("independent"), "keep").unwrap();
        pairs.push((p("independent"), p("done")));
        let (plan, rejections) = Renamer::from_iter(pairs).prepare().into_parts();
        assert_eq!(plan.len(), 1);
        assert_eq!(
            rejections.iter().map(|r| r.renames.len()).sum::<usize>(),
            128
        );
        assert_eq!(rejections[0].renames.len(), 64);
        assert!(matches!(
            &rejections[0].reason,
            RejectionReason::Plan(PlanError::DuplicateSource { .. })
        ));
        for rejection in &rejections[1..] {
            assert_eq!(rejection.renames.len(), 1);
            assert!(matches!(&rejection.reason,
                RejectionReason::Plan(PlanError::OverlappingPaths { descendant_path, .. })
                    if descendant_path == &rejection.renames[0].source));
        }
        plan.apply().unwrap();
        assert_eq!(fs::read_to_string(p("done")).unwrap(), "keep");
        assert_eq!(fs::read_dir(p("folder")).unwrap().count(), 64);
    }
}

#[test]
fn resolution_errors_do_not_hide_duplicates_or_overlaps() {
    // Exercise both parent resolution and final-entry identification failures.
    for bad in ["missing/../bad", "bad\0"] {
        let cases = [
            (("a", bad), ("a", "good")),
            ((bad, "out"), ("a", "out")),
            (("folder/child", bad), ("folder", "moved")),
            (("folder", bad), ("folder/child", "good")),
            ((bad, "out/child"), ("a", "out")),
            ((bad, "out"), ("a", "out/child")),
            (("folder", bad), ("a", "folder/new")),
            ((bad, "folder"), ("folder/child", "good")),
        ];
        for (case, (invalid, conflicting)) in cases.into_iter().enumerate() {
            for reverse in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let p = |name| dir.path().join(name);
                fs::create_dir(p("folder")).unwrap();
                for name in ["a", "folder/child", "independent"] {
                    fs::write(p(name), name).unwrap();
                }
                let mut pairs = vec![
                    (p(invalid.0), p(invalid.1)),
                    (p(conflicting.0), p(conflicting.1)),
                    (p("independent"), p("done")),
                ];
                if reverse {
                    pairs.reverse();
                }
                let report = Renamer::from_iter(pairs.clone()).prepare();
                let (plan, rejections) = report.into_parts();
                assert_eq!(plan.len(), 1, "case {case}, reverse={reverse}");
                let rejected: Vec<_> = rejections.iter().flat_map(|r| &r.renames).collect();
                assert_eq!(rejected.len(), 2);
                for pair in [
                    (p(invalid.0), p(invalid.1)),
                    (p(conflicting.0), p(conflicting.1)),
                ] {
                    assert_eq!(
                        rejected
                            .iter()
                            .filter(|r| r.source == pair.0 && r.target == pair.1)
                            .count(),
                        1
                    );
                }
                assert!(matches!(
                    &rejections[0].reason,
                    RejectionReason::Plan(PlanError::ResolvePath { .. })
                ));
                assert!(
                    match &rejections[1].reason {
                        RejectionReason::Plan(PlanError::DuplicateSource { .. }) => case == 0,
                        RejectionReason::Plan(PlanError::DuplicateTarget { .. }) => case == 1,
                        RejectionReason::Plan(PlanError::OverlappingPaths { .. }) => case >= 2,
                        _ => false,
                    },
                    "case {case}, reverse={reverse}, bad={bad:?}"
                );
                assert!(Renamer::from_iter(pairs).prepare().into_plan().is_err());
                plan.apply().unwrap();
                for name in ["a", "folder/child"] {
                    assert_eq!(fs::read_to_string(p(name)).unwrap(), name);
                }
                assert_eq!(fs::read_to_string(p("done")).unwrap(), "independent");
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn resolution_errors_keep_parent_aliases_and_dependent_renames_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir(p("real")).unwrap();
    std::os::unix::fs::symlink("real", p("alias")).unwrap();
    for name in ["real/a", "b", "independent"] {
        fs::write(p(name), name).unwrap();
    }
    let (plan, rejected) = Renamer::from_iter([
        (p("alias/a"), p("missing/../bad")),
        (p("real/a"), p("good")),
        (p("b"), p("real/a")),
        (p("independent"), p("done")),
    ])
    .prepare()
    .into_parts();
    assert_eq!(rejected.iter().map(|r| r.renames.len()).sum::<usize>(), 3);
    assert_eq!(plan.len(), 1);
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("real/a")).unwrap(), "real/a");
    assert_eq!(fs::read_to_string(p("b")).unwrap(), "b");
    assert_eq!(fs::read_to_string(p("done")).unwrap(), "independent");
}

#[test]
fn failed_identification_keeps_its_own_resolved_path_for_overlap_checks() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    for name in ["a", "b", "independent"] {
        fs::write(p(name), name).unwrap();
    }
    let (plan, rejections) = Renamer::from_iter([
        (p("a"), p("out/child\0")),
        (p("b"), p("out")),
        (p("independent"), p("done")),
    ])
    .prepare()
    .into_parts();
    assert_eq!(plan.len(), 1);
    assert_eq!(rejections.len(), 2);
    assert!(matches!(
        &rejections[0].reason,
        RejectionReason::Plan(PlanError::ResolvePath { path, .. })
            if path == &p("out/child\0")
    ));
    assert!(matches!(
        &rejections[1].reason,
        RejectionReason::Plan(PlanError::OverlappingPaths { ancestor_path, descendant_path })
            if ancestor_path == &p("out") && descendant_path == &p("out/child\0")
    ));
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("done")).unwrap(), "independent");
    assert!(p("a").exists());
    assert!(p("b").exists());
    assert!(!p("out").exists());
}

#[test]
fn shared_failures_prefer_source_errors_regardless_of_inspection_phase() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    let pairs = [
        (p("bad\0"), p("missing/../bad")),
        (p("a"), p("bad\0")),
        (p("b"), p("bad\0")),
        (p("missing/../bad"), p("bad\0")),
    ];
    let (plan, rejections) = Renamer::from_iter(pairs.clone()).prepare().into_parts();
    assert!(plan.is_empty());
    assert_eq!(rejections.len(), pairs.len());
    for (index, rejection) in rejections.iter().enumerate() {
        assert_eq!(rejection.renames.len(), 1);
        assert_eq!(rejection.renames[0].source, pairs[index].0);
        assert_eq!(rejection.renames[0].target, pairs[index].1);
        let RejectionReason::Plan(PlanError::ResolvePath { path, source }) = &rejection.reason
        else {
            panic!("expected a path diagnostic");
        };
        let expected = if index == 3 {
            "missing/../bad"
        } else {
            "bad\0"
        };
        assert_eq!(path, &p(expected));
        let expected_kind = if index == 3 {
            std::io::ErrorKind::NotFound
        } else {
            std::io::ErrorKind::InvalidInput
        };
        assert_eq!(source.kind(), expected_kind);
    }
}

#[test]
fn alias_noops_are_discarded_before_identifying_invalid_final_names() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir(p("sub")).unwrap();
    fs::write(p("a"), "A").unwrap();
    let plan = Renamer::from_iter([(p("sub/../bad\0"), p("bad\0")), (p("a"), p("b"))])
        .prepare()
        .into_plan()
        .unwrap();
    assert_eq!(plan.len(), 1);
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("b")).unwrap(), "A");
}

#[cfg(unix)]
#[test]
fn path_and_inspection_errors_include_operations_and_do_not_abort_the_batch() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    for name in ["a", "b", "c", "blocked"] {
        fs::write(p(name), name).unwrap();
    }
    let report = Renamer::from_iter([
        (p("a"), p("missing/../target")),
        (p("b"), p("blocked/")),
        (p("c"), p("good")),
    ])
    .prepare();
    assert_eq!(
        report.rejections().iter().flat_map(|r| &r.renames).count(),
        2
    );
    assert!(matches!(&report.rejections()[0].reason,
        RejectionReason::Plan(PlanError::ResolvePath { path, .. }) if path == &p("missing/../target")));
    let resolved_root = dir.path().canonicalize().unwrap();
    assert!(matches!(&report.rejections()[1].reason,
        RejectionReason::Filesystem(FsError::Inspect { source_path, target_path, .. })
            if source_path == &resolved_root.join("b")
                && target_path.as_os_str() == resolved_root.join("blocked/").as_os_str()));
    assert_eq!(report.rejections()[1].renames[0].source, p("b"));
    report.into_parts().0.apply().unwrap();
    assert_eq!(fs::read_to_string(p("good")).unwrap(), "c");
    assert!(!p("missing").exists());
}

#[test]
fn alias_noops_and_resolution_errors_preserve_an_independent_chain() {
    for reverse in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let p = |name| dir.path().join(name);
        fs::create_dir(p("sub")).unwrap();
        fs::write(p("a"), "A").unwrap();
        fs::write(p("b"), "B").unwrap();
        let mut pairs = [
            (p("sub/../b"), p("b")), // Parent alias: a no-op.
            (p("a"), p("b")),
            (p("b"), p("c")),
            (p("unrelated"), p("missing/../invalid")),
        ];
        if reverse {
            pairs.reverse();
        }
        let (plan, rejections) = Renamer::from_iter(pairs).prepare().into_parts();
        assert_eq!(plan.len(), 2);
        assert_eq!(rejections.len(), 1);
        assert_eq!(rejections[0].renames.len(), 1);
        assert_eq!(rejections[0].renames[0].source, p("unrelated"));
        assert!(matches!(
            &rejections[0].reason,
            RejectionReason::Plan(PlanError::ResolvePath { .. })
        ));
        plan.apply().unwrap();
        assert_eq!(fs::read_to_string(p("b")).unwrap(), "A");
        assert_eq!(fs::read_to_string(p("c")).unwrap(), "B");
        assert!(!p("missing").exists());
    }
}

#[test]
fn recheck_removes_new_conflicts_and_their_dependents() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    for name in ["a", "b", "other"] {
        fs::write(p(name), name).unwrap();
    }
    let mut plan =
        Renamer::from_iter([(p("a"), p("b")), (p("b"), p("c")), (p("other"), p("free"))])
            .prepare()
            .into_plan()
            .unwrap();
    fs::write(p("c"), "new arrival").unwrap();
    let rejections = plan.reject_conflicts();
    assert_eq!(rejections.len(), 2);
    assert_eq!(rejections[0].renames[0].source, p("b"));
    assert_eq!(rejections[1].renames[0].source, p("a"));
    let retained: Vec<_> = plan
        .iter()
        .map(|r| (r.source.original, r.target.original))
        .collect();
    assert_eq!(retained, [(&p("other"), &p("free"))]);
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("c")).unwrap(), "new arrival");
    assert_eq!(fs::read_to_string(p("free")).unwrap(), "other");
}

#[test]
fn report_preserves_owned_payloads_without_requiring_clone() {
    #[derive(Debug)]
    struct Source {
        path: PathBuf,
        id: usize,
    }
    impl AsRef<std::path::Path> for Source {
        fn as_ref(&self) -> &std::path::Path {
            &self.path
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::write(p("c"), "contents").unwrap();
    let report = Renamer::from_iter([
        (
            Source {
                path: p("a"),
                id: 7,
            },
            p("target"),
        ),
        (
            Source {
                path: p("b"),
                id: 8,
            },
            p("target"),
        ),
        (
            Source {
                path: p("c"),
                id: 9,
            },
            p("free"),
        ),
    ])
    .prepare();
    let (plan, rejections) = report.into_parts();
    let ids: Vec<_> = rejections
        .into_iter()
        .flat_map(|r| r.renames)
        .map(|r| r.source.id)
        .collect();
    assert_eq!(ids, [7, 8]);
    assert_eq!(plan.iter().next().unwrap().source.original.id, 9);
    let completed: Vec<_> = plan.apply_iter().map(Result::unwrap).collect();
    assert_eq!(completed[0].source.id, 9);
    assert_eq!(fs::read_to_string(p("free")).unwrap(), "contents");
}

#[cfg(feature = "unicode")]
#[test]
fn equivalent_natural_names_have_a_deterministic_lexical_tie_break() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::write(p("a"), "A").unwrap();
    fs::write(p("b"), "B").unwrap();
    let pairs = [(p("a"), p("photo1.jpg")), (p("b"), p("photo01.jpg"))];
    let display = |pairs| {
        let plan = Renamer::from_iter(pairs).prepare().into_plan().unwrap();
        let mut bytes = Vec::new();
        plan.write_to(&mut bytes).unwrap();
        bytes
    };
    assert_eq!(
        display(pairs.clone()),
        display([pairs[1].clone(), pairs[0].clone()])
    );
}
