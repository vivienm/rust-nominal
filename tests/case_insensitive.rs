//! Run on an insensitive filesystem, or set NOMINAL_CASE_INSENSITIVE_DIR to one.

use std::fs;

use nominal::{PlanError, Renamer};

fn case_insensitive_dir() -> Option<tempfile::TempDir> {
    let configured = std::env::var_os("NOMINAL_CASE_INSENSITIVE_DIR");
    let dir = match &configured {
        Some(root) => tempfile::tempdir_in(root).unwrap(),
        None => tempfile::tempdir().unwrap(),
    };
    fs::write(dir.path().join("probe"), "probe").unwrap();
    let insensitive = dir.path().join("PROBE").exists();
    if configured.is_some() {
        assert!(insensitive, "NOMINAL_CASE_INSENSITIVE_DIR must ignore case");
    }
    fs::remove_file(dir.path().join("probe")).unwrap();
    insensitive.then_some(dir)
}

#[test]
fn case_alias_chains_vacate_targets_first() {
    for check in [false, true] {
        let Some(dir) = case_insensitive_dir() else {
            return;
        };
        let p = |name: &str| dir.path().join(name);
        fs::write(p("a"), "A").unwrap();
        fs::write(p("b"), "B").unwrap();
        let mut plan = Renamer::from_iter([(p("a"), p("b")), (p("B"), p("c"))])
            .plan()
            .unwrap();
        if check {
            assert!(plan.check_fs().unwrap().is_empty());
        }
        let results: Vec<_> = plan.apply_iter().map(Result::unwrap).collect();
        assert_eq!(results[0].source, p("B"));
        assert!(!p("a").exists());
        assert_eq!(fs::read_to_string(p("b")).unwrap(), "A");
        assert_eq!(fs::read_to_string(p("c")).unwrap(), "B");
    }
}

#[test]
fn case_alias_conflicts_propagate() {
    let Some(dir) = case_insensitive_dir() else {
        return;
    };
    let p = |name: &str| dir.path().join(name);
    for name in ["a", "b", "c"] {
        fs::write(p(name), name).unwrap();
    }
    let mut plan = Renamer::from_iter([(p("a"), p("b")), (p("B"), p("c"))])
        .plan()
        .unwrap();
    assert_eq!(plan.check_fs().unwrap().len(), 2);
    assert!(plan.is_empty());
    for name in ["a", "b", "c"] {
        assert_eq!(fs::read_to_string(p(name)).unwrap(), name);
    }
}

#[test]
fn existing_case_aliases_reject_duplicates_and_cycles() {
    let Some(dir) = case_insensitive_dir() else {
        return;
    };
    let p = |name: &str| dir.path().join(name);
    fs::write(p("a"), "A").unwrap();
    fs::write(p("b"), "B").unwrap();
    let plan =
        |pairs: [(&str, &str); 2]| Renamer::from_iter(pairs.map(|(a, b)| (p(a), p(b)))).plan();
    assert!(matches!(
        plan([("a", "c"), ("A", "d")]),
        Err(PlanError::DuplicateSource { .. })
    ));
    assert!(matches!(
        plan([("a", "b"), ("c", "B")]),
        Err(PlanError::DuplicateTarget { .. })
    ));
    assert!(matches!(
        plan([("a", "b"), ("B", "A")]),
        Err(PlanError::Cycle { .. })
    ));
}

#[test]
fn case_only_rename_is_not_a_cycle_or_noop() {
    let Some(dir) = case_insensitive_dir() else {
        return;
    };
    let p = |name: &str| dir.path().join(name);
    fs::write(p("lower"), "data").unwrap();
    let mut plan = Renamer::from_iter([(p("lower"), p("LOWER"))])
        .plan()
        .unwrap();
    assert_eq!(plan.len(), 1);
    assert!(plan.check_fs().unwrap().is_empty());
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("LOWER")).unwrap(), "data");
    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["LOWER"]);
}

#[test]
fn parent_case_aliases_share_missing_destinations() {
    let Some(dir) = case_insensitive_dir() else {
        return;
    };
    let p = |name: &str| dir.path().join(name);
    fs::create_dir(p("folder")).unwrap();
    let error = Renamer::from_iter([
        (p("a"), p("folder/new/file")),
        (p("b"), p("FOLDER/new/file")),
    ])
    .plan()
    .unwrap_err();
    assert!(matches!(error, PlanError::DuplicateTarget { .. }));
    let error = Renamer::from_iter([(p("folder"), p("moved")), (p("b"), p("FOLDER/new/file"))])
        .plan()
        .unwrap_err();
    assert!(matches!(error, PlanError::OverlappingPaths { .. }));
}

#[cfg(unix)]
#[test]
fn dangling_symlink_case_aliases_form_chains() {
    use std::os::unix::fs::symlink;
    let Some(dir) = case_insensitive_dir() else {
        return;
    };
    let p = |name: &str| dir.path().join(name);
    fs::write(p("a"), "A").unwrap();
    symlink("missing", p("b")).unwrap();
    let mut plan = Renamer::from_iter([(p("a"), p("b")), (p("B"), p("c"))])
        .plan()
        .unwrap();
    assert!(plan.check_fs().unwrap().is_empty());
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("b")).unwrap(), "A");
    assert_eq!(
        fs::read_link(p("c")).unwrap(),
        std::path::Path::new("missing")
    );
}

#[test]
fn case_alias_parents_do_not_create_false_conflicts() {
    let Some(dir) = case_insensitive_dir() else {
        return;
    };
    let p = |name| dir.path().join(name);
    fs::create_dir(p("folder")).unwrap();
    fs::write(p("folder/a"), "data").unwrap();
    let mut plan = Renamer::from_iter([(p("folder/a"), p("FOLDER/A"))])
        .plan()
        .unwrap();
    assert!(plan.check_fs().unwrap().is_empty());
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("folder/A")).unwrap(), "data");
}

#[test]
fn case_only_directory_rename_preserves_contents_and_spelling() {
    let Some(dir) = case_insensitive_dir() else {
        return;
    };
    let p = |name| dir.path().join(name);
    fs::create_dir(p("folder")).unwrap();
    fs::write(p("folder/file"), "data").unwrap();
    nominal::Rename::new(p("folder"), p("FOLDER"))
        .apply()
        .unwrap();
    assert_eq!(fs::read_to_string(p("FOLDER/file")).unwrap(), "data");
    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["FOLDER"]);
}

#[cfg(unix)]
#[test]
fn case_only_symlink_rename_preserves_the_link_itself() {
    use std::os::unix::fs::symlink;
    for referent in ["missing", "file"] {
        let Some(dir) = case_insensitive_dir() else {
            return;
        };
        let p = |name| dir.path().join(name);
        fs::write(p("file"), "data").unwrap();
        symlink(referent, p("link")).unwrap();
        nominal::Rename::new(p("link"), p("LINK")).apply().unwrap();
        assert_eq!(
            fs::read_link(p("LINK")).unwrap(),
            std::path::Path::new(referent)
        );
        let mut names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["LINK", "file"]);
        assert_eq!(fs::read_to_string(p("file")).unwrap(), "data");
    }
}

#[cfg(unix)]
#[test]
fn invalid_case_alias_suffixes_leave_directory_contents_in_place() {
    for (source, target) in [("folder/.", "FOLDER"), ("folder", "FOLDER/.")] {
        let Some(dir) = case_insensitive_dir() else {
            return;
        };
        let p = |name: &str| dir.path().join(name);
        fs::create_dir(p("folder")).unwrap();
        fs::write(p("folder/file"), "data").unwrap();
        assert!(nominal::Rename::new(p(source), p(target)).apply().is_err());
        assert_eq!(fs::read_to_string(p("folder/file")).unwrap(), "data");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["folder"]);
    }
}

#[cfg(unix)]
#[test]
fn case_only_directory_rename_does_not_require_writing_its_contents() {
    use std::os::unix::fs::PermissionsExt;
    let Some(dir) = case_insensitive_dir() else {
        return;
    };
    let source = dir.path().join("folder");
    let target = dir.path().join("FOLDER");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), "data").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();
    let result = nominal::Rename::new(&source, &target).apply();
    // Restore permissions for TempDir cleanup, even if the assertion fails.
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    result.unwrap();
    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["FOLDER"]);
    assert_eq!(fs::read_to_string(target.join("file")).unwrap(), "data");
}
