//! Filesystem safety regressions exercised through the public API.

use std::{fs, path::Path};

use nominal::{FsConflict, RenameError, Renamer};

fn assert_conflict(source: &Path, target: &Path) {
    let mut plan = Renamer::from_iter([(source, target)]).plan().unwrap();
    let conflicts = plan.check_fs().unwrap();
    assert!(matches!(
        conflicts.as_slice(),
        [FsConflict::TargetExists { target_path }] if target_path == target
    ));
    assert!(plan.is_empty());
    let error = Renamer::from_iter([(source, target)])
        .plan()
        .unwrap()
        .apply()
        .unwrap_err();
    assert!(matches!(error.source, RenameError::TargetExists));
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
    let mut plan = Renamer::from_iter([(&source, &target)]).plan().unwrap();
    assert!(plan.check_fs().unwrap().is_empty());
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
        let mut plan = Renamer::from_iter([(&source, &target)]).plan().unwrap();
        assert!(plan.check_fs().unwrap().is_empty());
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
        let mut plan = Renamer::from_iter([(&source, &target)]).plan().unwrap();
        assert!(plan.check_fs().unwrap().is_empty());
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
        let mut plan = Renamer::from_iter([(&source, &target)]).plan().unwrap();
        assert!(plan.check_fs().unwrap().is_empty());
        plan.apply().unwrap();
        assert!(!source.exists());
        assert!(target.exists());
    }
}
