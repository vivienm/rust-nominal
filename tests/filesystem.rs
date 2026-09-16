//! Filesystem safety regressions exercised through the public API.

use std::{fs, path::Path};

use nominal::{FsError, RejectionReason, Rename, RenameError, Renamer};

#[test]
fn direct_rename_rejects_missing_parent_before_dotdot_without_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let victim = dir.path().join("victim");
    fs::write(&source, "new contents").unwrap();
    fs::write(&victim, "keep me").unwrap();

    let result = Rename::new(&source, dir.path().join("missing/../victim")).apply();

    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&source).unwrap(), "new contents");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep me");
    assert!(!dir.path().join("missing").exists());
}

#[test]
fn direct_rename_resolves_existing_parents_and_creates_missing_destinations() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    let source = dir.path().join("source");
    fs::write(&source, "contents").unwrap();

    Rename::new(&source, dir.path().join("sub/../new/deep/target"))
        .apply()
        .unwrap();

    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("new/deep/target")).unwrap(),
        "contents"
    );
}

#[test]
fn prepared_execution_handles_removed_or_blocked_target_parents() {
    for blocked in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let parent = dir.path().join("parent");
        let target = parent.join("target");
        fs::write(&source, "contents").unwrap();
        fs::create_dir(&parent).unwrap();
        let plan = Renamer::from_iter([(&source, &target)])
            .prepare()
            .into_plan()
            .unwrap();
        fs::remove_dir(&parent).unwrap();
        if blocked {
            fs::write(&parent, "blocking file").unwrap();
            assert!(plan.apply().is_err());
            assert_eq!(fs::read_to_string(&source).unwrap(), "contents");
            assert_eq!(fs::read_to_string(&parent).unwrap(), "blocking file");
        } else {
            plan.apply().unwrap();
            assert_eq!(fs::read_to_string(&target).unwrap(), "contents");
            assert!(!source.exists());
        }
    }
}

fn assert_conflict(source: &Path, target: &Path) {
    let report = Renamer::from_iter([(source, target)]).prepare();
    assert_eq!(
        report.rejections().iter().flat_map(|r| &r.renames).count(),
        1
    );
    let resolved_target = target
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .join(target.file_name().unwrap());
    assert!(matches!(&report.rejections()[0].reason,
        RejectionReason::Filesystem(FsError::TargetExists { target_path }) if target_path == &resolved_target));
    assert!(report.into_plan().is_err());
    assert!(matches!(
        Rename::new(source, target).apply(),
        Err(RenameError::TargetExists)
    ));
}

#[test]
fn missing_source_does_not_hide_existing_target() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("missing");
    let target = dir.path().join("target");
    fs::write(&target, "keep me").unwrap();
    assert_conflict(&source, &target);
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
}

#[test]
fn distinct_hard_links_are_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    fs::write(&source, "keep me").unwrap();
    fs::hard_link(&source, &target).unwrap();
    assert_conflict(&source, &target);
    assert!(source.exists());
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
}

#[test]
fn case_only_rename_preserves_stored_spelling() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("lower");
    let target = dir.path().join("LOWER");
    fs::write(&source, "contents").unwrap();
    // Exercises the existing-target exception on case-insensitive filesystems,
    // and the ordinary absent-target path on case-sensitive ones.
    let mut plan = Renamer::from_iter([(&source, &target)])
        .prepare()
        .into_plan()
        .unwrap();
    assert!(plan.reject_conflicts().is_empty());
    plan.apply().unwrap();
    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["LOWER"]);
    assert_eq!(fs::read_to_string(&target).unwrap(), "contents");
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    };

    #[test]
    fn prepared_execution_keeps_captured_paths_when_original_alias_changes() {
        let dir = tempfile::tempdir().unwrap();
        let p = |name| dir.path().join(name);
        fs::create_dir(p("original")).unwrap();
        fs::create_dir(p("alternate")).unwrap();
        fs::write(p("original/source"), "planned contents").unwrap();
        fs::write(p("alternate/source"), "unrelated contents").unwrap();
        symlink("original", p("alias")).unwrap();
        let plan = Renamer::from_iter([(p("alias/source"), p("alias/target"))])
            .prepare()
            .into_plan()
            .unwrap();
        fs::remove_file(p("alias")).unwrap();
        symlink("alternate", p("alias")).unwrap();

        plan.apply().unwrap();

        assert!(!p("original/source").exists());
        assert_eq!(
            fs::read_to_string(p("original/target")).unwrap(),
            "planned contents"
        );
        assert_eq!(
            fs::read_to_string(p("alternate/source")).unwrap(),
            "unrelated contents"
        );
        assert!(!p("alternate/target").exists());
    }

    #[test]
    fn prepared_execution_rechecks_entries_when_captured_parent_becomes_a_symlink() {
        for occupied in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let p = |name| dir.path().join(name);
            fs::create_dir(p("parent")).unwrap();
            fs::create_dir(p("replacement")).unwrap();
            fs::write(p("parent/source"), "original contents").unwrap();
            fs::write(p("replacement/source"), "current contents").unwrap();
            let plan = Renamer::from_iter([(p("parent/source"), p("parent/target"))])
                .prepare()
                .into_plan()
                .unwrap();
            fs::rename(p("parent"), p("saved")).unwrap();
            symlink("replacement", p("parent")).unwrap();

            if occupied {
                fs::write(p("replacement/target"), "keep me").unwrap();
                assert!(matches!(
                    plan.apply().unwrap_err().source,
                    RenameError::TargetExists
                ));
                assert_eq!(
                    fs::read_to_string(p("replacement/target")).unwrap(),
                    "keep me"
                );
                assert_eq!(
                    fs::read_to_string(p("replacement/source")).unwrap(),
                    "current contents"
                );
            } else {
                // Captured spellings do not pin directories or source identities.
                plan.apply().unwrap();
                assert_eq!(
                    fs::read_to_string(p("replacement/target")).unwrap(),
                    "current contents"
                );
                assert!(!p("replacement/source").exists());
            }
            assert_eq!(
                fs::read_to_string(p("saved/source")).unwrap(),
                "original contents"
            );
            assert!(!p("saved/target").exists());
        }
    }

    #[test]
    fn prepared_execution_preserves_a_new_dangling_parent_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let p = |name| dir.path().join(name);
        fs::write(p("source"), "contents").unwrap();
        let plan = Renamer::from_iter([(p("source"), p("parent/target"))])
            .prepare()
            .into_plan()
            .unwrap();
        symlink("missing", p("parent")).unwrap();

        assert!(plan.apply().is_err());
        assert_eq!(fs::read_to_string(p("source")).unwrap(), "contents");
        assert_eq!(fs::read_link(p("parent")).unwrap(), Path::new("missing"));
        assert!(!p("missing").exists());
    }

    #[test]
    fn dangling_source_cannot_overwrite_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        symlink("missing", &source).unwrap();
        fs::write(&target, "keep me").unwrap();
        assert_conflict(&source, &target);
        assert_eq!(fs::read_link(&source).unwrap(), Path::new("missing"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
    }

    #[test]
    fn source_symlink_cannot_overwrite_its_referent() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&target, "keep me").unwrap();
        symlink(&target, &source).unwrap();
        assert_conflict(&source, &target);
        assert_eq!(fs::read_link(&source).unwrap(), target);
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
    }

    #[test]
    fn target_symlinks_are_preserved_including_dangling_ones() {
        for referent in ["missing", "source"] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source");
            let target = dir.path().join("target");
            fs::write(&source, "contents").unwrap();
            symlink(referent, &target).unwrap();
            assert_conflict(&source, &target);
            assert_eq!(fs::read_link(&target).unwrap(), Path::new(referent));
            assert_eq!(fs::read_to_string(&source).unwrap(), "contents");
        }
    }

    #[test]
    fn dangling_source_can_be_moved_to_a_free_target() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        symlink("missing", &source).unwrap();
        let mut plan = Renamer::from_iter([(&source, &target)])
            .prepare()
            .into_plan()
            .unwrap();
        assert!(plan.reject_conflicts().is_empty());
        plan.apply().unwrap();
        assert!(fs::symlink_metadata(&source).is_err());
        assert_eq!(fs::read_link(&target).unwrap(), Path::new("missing"));
    }

    #[test]
    fn rename_does_not_require_read_access() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, "contents").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o000)).unwrap();
        let mut plan = Renamer::from_iter([(&source, &target)])
            .prepare()
            .into_plan()
            .unwrap();
        assert!(plan.reject_conflicts().is_empty());
        plan.apply().unwrap();
        assert!(!source.exists());
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0
        );
    }

    #[test]
    fn sockets_can_be_renamed_without_opening_them() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        let _listener = UnixListener::bind(&source).unwrap();
        let mut plan = Renamer::from_iter([(&source, &target)])
            .prepare()
            .into_plan()
            .unwrap();
        assert!(plan.reject_conflicts().is_empty());
        plan.apply().unwrap();
        assert!(!source.exists());
        assert!(target.exists());
    }
}

#[test]
fn conflicts_propagate_through_chains_without_removing_independent_moves() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    for name in ["a", "b", "c", "d", "independent"] {
        fs::write(p(name), name).unwrap();
    }
    let (mut plan, conflicts) = Renamer::from_iter([
        (p("a"), p("b")),
        (p("b"), p("c")),
        (p("c"), p("d")),
        (p("independent"), p("free")),
    ])
    .prepare()
    .into_parts();
    let targets: Vec<_> = conflicts
        .iter()
        .map(|conflict| match &conflict.reason {
            RejectionReason::Filesystem(FsError::TargetExists { target_path }) => {
                target_path.clone()
            }
            _ => panic!("unexpected conflict: {conflict:?}"),
        })
        .collect();
    let resolved_root = dir.path().canonicalize().unwrap();
    assert_eq!(
        targets,
        ["d", "c", "b"].map(|name| resolved_root.join(name))
    );
    assert_eq!(plan.len(), 1);
    assert!(plan.reject_conflicts().is_empty());
    plan.apply().unwrap();
    for name in ["a", "b", "c", "d"] {
        assert_eq!(fs::read_to_string(p(name)).unwrap(), name);
    }
    assert_eq!(fs::read_to_string(p("free")).unwrap(), "independent");
}

#[test]
fn hard_link_conflict_propagates_to_dependent_moves() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::write(p("a"), "A").unwrap();
    fs::write(p("b"), "B").unwrap();
    fs::hard_link(p("b"), p("c")).unwrap();
    let (plan, conflicts) = Renamer::from_iter([(p("a"), p("b")), (p("b"), p("c"))])
        .prepare()
        .into_parts();
    assert_eq!(conflicts.len(), 2);
    assert!(plan.is_empty());
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(p("a")).unwrap(), "A");
    assert_eq!(fs::read_to_string(p("b")).unwrap(), "B");
    assert_eq!(fs::read_to_string(p("c")).unwrap(), "B");
}

#[test]
fn unblocked_chain_moves_original_contents_to_their_targets() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::write(p("a"), "A").unwrap();
    fs::write(p("b"), "B").unwrap();
    let mut plan = Renamer::from_iter([(p("a"), p("b")), (p("b"), p("c"))])
        .prepare()
        .into_plan()
        .unwrap();
    assert!(plan.reject_conflicts().is_empty());
    assert_eq!(plan.len(), 2);
    plan.apply().unwrap();
    assert!(!p("a").exists());
    assert_eq!(fs::read_to_string(p("b")).unwrap(), "A");
    assert_eq!(fs::read_to_string(p("c")).unwrap(), "B");
}

#[test]
fn separate_hard_links_can_be_moved_independently() {
    let dir = tempfile::tempdir().unwrap();
    let p = |name| dir.path().join(name);
    fs::write(p("a"), "data").unwrap();
    fs::hard_link(p("a"), p("b")).unwrap();
    let mut plan = Renamer::from_iter([(p("a"), p("c")), (p("b"), p("d"))])
        .prepare()
        .into_plan()
        .unwrap();
    assert!(plan.reject_conflicts().is_empty());
    plan.apply().unwrap();
    assert!(!p("a").exists());
    assert!(!p("b").exists());
    assert_eq!(fs::read_to_string(p("c")).unwrap(), "data");
    assert_eq!(fs::read_to_string(p("d")).unwrap(), "data");
}
