//! Strict and best-effort consumption of a single preparation report.

use std::{fs, path::PathBuf};

#[cfg(unix)]
use nominal::FsError;
use nominal::{PlanError, RejectionReason, Renamer};

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
    assert_eq!(report.rejected_count(), 5);
    assert_eq!(report.rejections().len(), 3);
    assert!(matches!(
        &report.rejections()[0].reason,
        RejectionReason::Plan(PlanError::DuplicateTarget { .. })
    ));
    let error = report.into_plan().unwrap_err();
    let message = error.to_string();
    for name in ["a", "b", "c", "d", "g"] {
        assert!(message.contains(p(name).to_str().unwrap()), "{message}");
    }
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
        assert_eq!(report.rejected_count(), 3);
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
fn overlaps_reject_all_affected_operations_but_keep_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir(p("folder")).unwrap();
    fs::write(p("folder/child"), "child").unwrap();
    fs::write(p("a"), "a").unwrap();
    let report = Renamer::from_iter([
        (p("folder"), p("moved")),
        (p("folder/child"), p("child")),
        (p("a"), p("b")),
    ])
    .prepare();
    assert_eq!(report.rejected_count(), 2);
    assert!(matches!(
        &report.rejections()[0].reason,
        RejectionReason::Plan(PlanError::OverlappingPaths { .. })
    ));
    report.into_parts().0.apply().unwrap();
    assert_eq!(fs::read_to_string(p("folder/child")).unwrap(), "child");
    assert_eq!(fs::read_to_string(p("b")).unwrap(), "a");
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
    assert_eq!(report.rejected_count(), 2);
    assert!(matches!(&report.rejections()[0].reason,
        RejectionReason::Plan(PlanError::ResolvePath { path, .. }) if path == &p("missing/../target")));
    assert!(matches!(&report.rejections()[1].reason,
        RejectionReason::Filesystem(FsError::Inspect { source_path, target_path, .. })
            if source_path == &p("b") && target_path.as_os_str() == p("blocked/").as_os_str()));
    assert_eq!(report.rejections()[1].renames[0].source, p("b"));
    report.into_parts().0.apply().unwrap();
    assert_eq!(fs::read_to_string(p("good")).unwrap(), "c");
    assert!(!p("missing").exists());
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
    ])
    .prepare();
    let (_, rejections) = report.into_parts();
    let ids: Vec<_> = rejections
        .into_iter()
        .flat_map(|r| r.renames)
        .map(|r| r.source.id)
        .collect();
    assert_eq!(ids, [7, 8]);
}

#[cfg(feature = "unicode")]
#[test]
fn equivalent_natural_names_have_a_deterministic_lexical_tie_break() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
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
