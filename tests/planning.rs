//! Planning regressions for path aliases and overlapping operations.

use std::fs;

use nominal::{PlanError, Renamer};

fn plan_error(error: nominal::PreparationError) -> PlanError {
    match error.rejections.into_iter().next().unwrap().reason {
        nominal::RejectionReason::Plan(error) => error,
        other => panic!("expected a planning diagnostic, got {other:?}"),
    }
}

#[test]
fn parent_destinations_are_supported_but_existing_files_are_protected() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("library");
    fs::create_dir(&library).unwrap();
    let source = library.join("input.epub");
    let target = library.join("../book.epub");
    fs::write(&source, "new book").unwrap();

    // Nominal is a general-purpose renamer; confinement is the caller's policy.
    let mut plan = Renamer::from_iter([(&source, &target)])
        .prepare()
        .into_plan()
        .unwrap();
    assert!(plan.reject_conflicts().is_empty());
    plan.apply().unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("book.epub")).unwrap(),
        "new book"
    );
    assert!(!source.exists());

    fs::write(&source, "another book").unwrap();
    let (plan, conflicts) = Renamer::from_iter([(&source, &target)])
        .prepare()
        .into_parts();
    assert_eq!(conflicts.len(), 1);
    assert!(plan.is_empty());
    assert!(
        Renamer::from_iter([(&source, &target)])
            .prepare()
            .into_plan()
            .is_err()
    );
    assert_eq!(fs::read_to_string(&source).unwrap(), "another book");
    assert_eq!(fs::read_to_string(&target).unwrap(), "new book");
}

#[test]
fn parent_aliases_form_a_valid_chain_with_or_without_recheck() {
    for check in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let p = |name| dir.path().join(name);
        fs::create_dir(p("sub")).unwrap();
        fs::write(p("a"), "A").unwrap();
        fs::write(p("b"), "B").unwrap();
        let mut plan = Renamer::from_iter([(p("a"), p("b")), (p("sub/../b"), p("c"))])
            .prepare()
            .into_plan()
            .unwrap();
        if check {
            assert!(plan.reject_conflicts().is_empty());
        }
        let results: Vec<_> = plan.apply_iter().map(Result::unwrap).collect();
        assert_eq!(results[0].source, p("sub/../b"));
        assert_eq!(fs::read_to_string(p("b")).unwrap(), "A");
        assert_eq!(fs::read_to_string(p("c")).unwrap(), "B");
        assert!(!p("a").exists());
    }
}

#[test]
fn duplicate_sources_and_targets_are_detected_through_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir(p("sub")).unwrap();
    let source_error = Renamer::from_iter([(p("a"), p("b")), (p("sub/../a"), p("c"))])
        .prepare()
        .into_plan()
        .map_err(plan_error)
        .unwrap_err();
    assert!(matches!(source_error, PlanError::DuplicateSource { .. }));
    let target_error = Renamer::from_iter([(p("a"), p("b")), (p("c"), p("sub/../b"))])
        .prepare()
        .into_plan()
        .map_err(plan_error)
        .unwrap_err();
    assert!(matches!(target_error, PlanError::DuplicateTarget { .. }));
}

#[test]
fn cycles_are_detected_through_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir(p("sub")).unwrap();
    let error = Renamer::from_iter([(p("a"), p("b")), (p("sub/../b"), p("a"))])
        .prepare()
        .into_plan()
        .map_err(plan_error)
        .unwrap_err();
    assert!(matches!(error, PlanError::Cycle { .. }));
}

#[test]
fn alias_noop_does_not_pretend_to_vacate_a_target() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir(p("sub")).unwrap();
    fs::write(p("a"), "A").unwrap();
    fs::write(p("b"), "B").unwrap();
    let (plan, conflicts) = Renamer::from_iter([(p("a"), p("b")), (p("b"), p("sub/../b"))])
        .prepare()
        .into_parts();
    assert_eq!(conflicts.len(), 1);
    assert!(plan.is_empty());
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("a")).unwrap(), "A");
    assert_eq!(fs::read_to_string(p("b")).unwrap(), "B");
}

#[test]
fn absent_target_directories_are_created_after_planning() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name: &str| dir.path().join(name);
    for name in ["a", "b", "c"] {
        fs::write(p(name), name).unwrap();
    }
    let mut plan =
        Renamer::from_iter(["a", "b", "c"].map(|name| (p(name), p(&format!("new/deep/{name}")))))
            .prepare()
            .into_plan()
            .unwrap();
    assert!(!p("new").exists());
    assert!(plan.reject_conflicts().is_empty());
    plan.apply().unwrap();
    for name in ["a", "b", "c"] {
        assert_eq!(
            fs::read_to_string(p(&format!("new/deep/{name}"))).unwrap(),
            name
        );
    }
}

#[test]
fn missing_parent_followed_by_dotdot_is_not_simplified() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::write(p("a"), "A").unwrap();
    // Resolving a missing parent for one endpoint must not make a subsequent
    // `..` through that parent valid, even when parent resolutions are cached.
    let error = Renamer::from_iter([(p("a"), p("missing/first")), (p("missing/../a"), p("b"))])
        .prepare()
        .into_plan()
        .map_err(plan_error)
        .unwrap_err();
    assert!(matches!(error, PlanError::ResolvePath { .. }));
    assert!(p("a").exists());
}

#[cfg(unix)]
#[test]
fn shared_parent_resolutions_are_not_reused_between_preparations() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let p = |name: &str| dir.path().join(name);
    for folder in ["first", "second"] {
        fs::create_dir(p(folder)).unwrap();
        for name in ["a", "b"] {
            fs::write(p(&format!("{folder}/{name}")), format!("{folder}/{name}")).unwrap();
        }
    }
    let pairs = || {
        ["a", "b"].map(|name| {
            (
                p(&format!("alias/{name}")),
                p(&format!("alias/{name}-renamed")),
            )
        })
    };
    symlink("first", p("alias")).unwrap();
    let first = Renamer::from_iter(pairs()).prepare().into_plan().unwrap();
    fs::remove_file(p("alias")).unwrap();
    symlink("second", p("alias")).unwrap();
    let second = Renamer::from_iter(pairs()).prepare().into_plan().unwrap();

    first.apply().unwrap();
    second.apply().unwrap();
    for folder in ["first", "second"] {
        for name in ["a", "b"] {
            assert!(!p(&format!("{folder}/{name}")).exists());
            assert_eq!(
                fs::read_to_string(p(&format!("{folder}/{name}-renamed"))).unwrap(),
                format!("{folder}/{name}"),
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn dotdot_after_a_symlink_uses_the_real_parent() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir_all(p("real/inner")).unwrap();
    symlink(p("real/inner"), p("alias")).unwrap();
    fs::write(p("a"), "A").unwrap();
    fs::write(p("real/b"), "B").unwrap();
    let mut plan = Renamer::from_iter([(p("a"), p("real/b")), (p("alias/../b"), p("c"))])
        .prepare()
        .into_plan()
        .unwrap();
    assert!(plan.reject_conflicts().is_empty());
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("real/b")).unwrap(), "A");
    assert_eq!(fs::read_to_string(p("c")).unwrap(), "B");
    assert!(p("alias").is_symlink());
}

#[cfg(unix)]
#[test]
fn dangling_parent_symlink_is_not_treated_as_a_future_directory() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    symlink("missing", p("parent")).unwrap();
    let error = Renamer::from_iter([(p("a"), p("parent/b"))])
        .prepare()
        .into_plan()
        .map_err(plan_error)
        .unwrap_err();
    assert!(matches!(error, PlanError::ResolvePath { .. }));
    assert_eq!(
        fs::read_link(p("parent")).unwrap(),
        std::path::Path::new("missing")
    );
}

#[cfg(unix)]
#[test]
fn normalization_preserves_trailing_directory_requirements() {
    for suffix in ["/", "/."] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("a");
        let target = dir.path().join("b");
        fs::write(&source, "A").unwrap();
        let with_suffix = dir.path().join(format!("a{suffix}"));
        let plan = Renamer::from_iter([(with_suffix, target.clone())])
            .prepare()
            .into_plan()
            .unwrap();
        assert!(plan.apply().is_err());
        assert_eq!(fs::read_to_string(&source).unwrap(), "A");
        assert!(!target.exists());
    }
}

#[test]
fn nested_destinations_are_rejected_before_any_files_are_moved() {
    for reverse in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let p = |name| dir.path().join(name);
        fs::write(p("a"), "A").unwrap();
        fs::write(p("b"), "B").unwrap();
        let mut renames = [(p("a"), p("out")), (p("b"), p("out/child"))];
        if reverse {
            renames.reverse();
        }
        let error = Renamer::from_iter(renames)
            .prepare()
            .into_plan()
            .map_err(plan_error)
            .unwrap_err();
        assert!(
            matches!(error, PlanError::OverlappingPaths { ancestor_path, descendant_path }
            if ancestor_path == p("out") && descendant_path == p("out/child"))
        );
        assert_eq!(fs::read_to_string(p("a")).unwrap(), "A");
        assert_eq!(fs::read_to_string(p("b")).unwrap(), "B");
        assert!(!p("out").exists());
    }
}

#[test]
fn nested_destinations_are_detected_through_parent_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir(p("sub")).unwrap();
    let error = Renamer::from_iter([(p("a"), p("out")), (p("b"), p("sub/../out/child"))])
        .prepare()
        .into_plan()
        .map_err(plan_error)
        .unwrap_err();
    assert!(matches!(error, PlanError::OverlappingPaths { .. }));
}

#[test]
fn directory_and_descendant_moves_require_separate_batches() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir(p("folder")).unwrap();
    fs::write(p("folder/file"), "data").unwrap();
    let error = Renamer::from_iter([(p("folder"), p("moved")), (p("folder/file"), p("file"))])
        .prepare()
        .into_plan()
        .map_err(plan_error)
        .unwrap_err();
    assert!(matches!(error, PlanError::OverlappingPaths { .. }));
    assert_eq!(fs::read_to_string(p("folder/file")).unwrap(), "data");
    let error = Renamer::from_iter([(p("folder"), p("folder/child"))])
        .prepare()
        .into_plan()
        .map_err(plan_error)
        .unwrap_err();
    assert!(matches!(error, PlanError::OverlappingPaths { .. }));
}

#[test]
fn sibling_destinations_and_independent_directory_moves_remain_supported() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir(p("folder")).unwrap();
    fs::write(p("folder/file"), "data").unwrap();
    fs::write(p("a"), "A").unwrap();
    fs::write(p("b"), "B").unwrap();
    let mut plan = Renamer::from_iter([
        (p("folder"), p("moved")),
        (p("a"), p("out/first")),
        (p("b"), p("out/second")),
    ])
    .prepare()
    .into_plan()
    .unwrap();
    assert!(plan.reject_conflicts().is_empty());
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("moved/file")).unwrap(), "data");
    assert_eq!(fs::read_to_string(p("out/first")).unwrap(), "A");
    assert_eq!(fs::read_to_string(p("out/second")).unwrap(), "B");
}

#[test]
fn relative_paths_are_anchored_at_planning_time() {
    const CHILD: &str = "NOMINAL_PLANNING_CWD_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // Changing cwd is process-wide; isolate this case from other tests.
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "relative_paths_are_anchored_at_planning_time"])
            .env(CHILD, "1")
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let base = std::env::current_dir().unwrap();
    fs::write("a", "A").unwrap();
    fs::write("b", "B").unwrap();
    fs::create_dir("other").unwrap();
    let mut plan = Renamer::from_iter([
        (std::path::PathBuf::from("a"), base.join("b")),
        (std::path::PathBuf::from("./b"), base.join("c")),
    ])
    .prepare()
    .into_plan()
    .unwrap();
    std::env::set_current_dir("other").unwrap();
    assert!(plan.reject_conflicts().is_empty());
    let results: Vec<_> = plan.apply_iter().map(Result::unwrap).collect();
    assert_eq!(results[0].source, std::path::Path::new("./b"));
    assert_eq!(fs::read_to_string(base.join("b")).unwrap(), "A");
    assert_eq!(fs::read_to_string(base.join("c")).unwrap(), "B");
    assert!(!base.join("a").exists());
    assert_eq!(fs::read_dir(".").unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn trailing_dot_components_do_not_turn_into_directory_moves() {
    for suffix in ["/.", "/./", "/././"] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("folder");
        let target = dir.path().join("moved");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file"), "data").unwrap();
        let with_suffix = dir.path().join(format!("folder{suffix}"));
        let plan = Renamer::from_iter([(with_suffix, target.clone())])
            .prepare()
            .into_plan()
            .unwrap();
        assert!(plan.apply().is_err());
        assert_eq!(fs::read_to_string(source.join("file")).unwrap(), "data");
        assert!(!target.exists());
    }
}

#[cfg(unix)]
#[test]
fn batch_cache_preserves_each_sources_directory_suffix() {
    for suffix in ["/", "/.", "/./"] {
        for reverse in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let p = |name: &str| dir.path().join(name);
            fs::write(p("a"), "A").unwrap();
            fs::write(p("b"), "B").unwrap();
            let mut renames = [(p("a"), p("b")), (p(&format!("b{suffix}")), p("c"))];
            if reverse {
                renames.reverse();
            }
            let plan = Renamer::from_iter(renames).prepare().into_plan().unwrap();
            assert!(plan.apply().is_err());
            assert_eq!(fs::read_to_string(p("a")).unwrap(), "A");
            assert_eq!(fs::read_to_string(p("b")).unwrap(), "B");
            assert!(!p("c").exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn batch_cache_preserves_each_targets_directory_suffix() {
    for suffix in ["/", "/.", "/./"] {
        for reverse in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let p = |name: &str| dir.path().join(name);
            fs::write(p("a"), "A").unwrap();
            fs::write(p("b"), "B").unwrap();
            let mut renames = [(p("b"), p("c")), (p("a"), p(&format!("b{suffix}")))];
            if reverse {
                renames.reverse();
            }
            let outcomes: Vec<_> = Renamer::from_iter(renames)
                .prepare()
                .into_plan()
                .unwrap()
                .apply_iter()
                .collect();
            assert!(outcomes[0].is_ok());
            assert!(outcomes[1].is_err());
            assert_eq!(fs::read_to_string(p("a")).unwrap(), "A");
            assert_eq!(fs::read_to_string(p("c")).unwrap(), "B");
        }
    }
}

#[cfg(unix)]
#[test]
fn directory_suffix_differences_are_not_discarded_as_noops() {
    for (source, target) in [("a", "a/"), ("a/.", "a")] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), "A").unwrap();
        let report =
            Renamer::from_iter([(dir.path().join(source), dir.path().join(target))]).prepare();
        assert_eq!(
            report.rejections().iter().flat_map(|r| &r.renames).count(),
            1
        );
        assert!(report.into_plan().is_err());
        assert_eq!(fs::read_to_string(dir.path().join("a")).unwrap(), "A");
    }
}
