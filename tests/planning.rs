//! Planning regressions for path aliases and overlapping operations.

use std::fs;

use nominal::{PlanError, Renamer};

#[test]
fn parent_aliases_form_a_valid_chain_with_or_without_check_fs() {
    for check in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let p = |name| dir.path().join(name);
        fs::create_dir(p("sub")).unwrap();
        fs::write(p("a"), "A").unwrap();
        fs::write(p("b"), "B").unwrap();
        let mut plan = Renamer::from_iter([(p("a"), p("b")), (p("sub/../b"), p("c"))])
            .plan()
            .unwrap();
        if check {
            assert!(plan.check_fs().unwrap().is_empty());
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
        .plan()
        .unwrap_err();
    assert!(matches!(source_error, PlanError::DuplicateSource { .. }));
    let target_error = Renamer::from_iter([(p("a"), p("b")), (p("c"), p("sub/../b"))])
        .plan()
        .unwrap_err();
    assert!(matches!(target_error, PlanError::DuplicateTarget { .. }));
}

#[test]
fn cycles_are_detected_through_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::create_dir(p("sub")).unwrap();
    let error = Renamer::from_iter([(p("a"), p("b")), (p("sub/../b"), p("a"))])
        .plan()
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
    let mut plan = Renamer::from_iter([(p("a"), p("b")), (p("b"), p("sub/../b"))])
        .plan()
        .unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan.check_fs().unwrap().len(), 1);
    assert!(plan.is_empty());
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("a")).unwrap(), "A");
    assert_eq!(fs::read_to_string(p("b")).unwrap(), "B");
}

#[test]
fn absent_target_directories_are_created_after_planning() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::write(p("a"), "A").unwrap();
    let mut plan = Renamer::from_iter([(p("a"), p("new/deep/b"))])
        .plan()
        .unwrap();
    assert!(!p("new").exists());
    assert!(plan.check_fs().unwrap().is_empty());
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("new/deep/b")).unwrap(), "A");
}

#[test]
fn missing_parent_followed_by_dotdot_is_not_simplified() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::write(p("a"), "A").unwrap();
    let error = Renamer::from_iter([(p("missing/../a"), p("b"))])
        .plan()
        .unwrap_err();
    assert!(matches!(error, PlanError::ResolvePath { .. }));
    assert!(p("a").exists());
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
        .plan()
        .unwrap();
    assert!(plan.check_fs().unwrap().is_empty());
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
        .plan()
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
            .plan()
            .unwrap();
        assert!(plan.apply().is_err());
        assert_eq!(fs::read_to_string(&source).unwrap(), "A");
        assert!(!target.exists());
    }
}
