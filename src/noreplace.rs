//! Platform primitives that never replace an existing destination.

use std::{io, path::Path};

use crate::RenameError;

pub(crate) fn rename(source: &Path, target: &Path) -> Result<(), RenameError> {
    native_rename(source, target).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            RenameError::TargetExists
        } else {
            RenameError::Io(error)
        }
    })
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn native_rename(source: &Path, target: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, source, CWD, target, RenameFlags::NOREPLACE).map_err(Into::into)
}

#[cfg(windows)]
fn native_rename(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut name: Vec<_> = path.as_os_str().encode_wide().collect();
        if name.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains NUL",
            ));
        }
        name.push(0);
        Ok(name)
    }
    let source = wide(source)?;
    let target = wide(target)?;
    // SAFETY: both buffers are NUL-terminated and live throughout the call.
    // Omitting REPLACE_EXISTING and COPY_ALLOWED prevents overwrites and
    // cross-volume copy/delete moves, including for temporary recovery.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    windows
)))]
fn native_rename(_source: &Path, _target: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic rename without replacement is not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Barrier, thread};

    #[test]
    fn native_rename_protects_existing_files_and_directories() {
        for directory in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source");
            let target = dir.path().join("target");
            if directory {
                fs::create_dir(&source).unwrap();
                fs::create_dir(&target).unwrap();
            } else {
                fs::write(&source, "source").unwrap();
                fs::write(&target, "target").unwrap();
            }
            // Bypass every preflight check: the system call must protect the
            // target even when it appears after the last occupancy check.
            assert!(super::rename(&source, &target).is_err());
            assert!(source.exists());
            if !directory {
                assert_eq!(fs::read_to_string(target).unwrap(), "target");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_rename_protects_dangling_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, "source").unwrap();
        std::os::unix::fs::symlink("missing", &target).unwrap();
        assert!(super::rename(&source, &target).is_err());
        assert_eq!(fs::read_to_string(source).unwrap(), "source");
        assert_eq!(
            fs::read_link(target).unwrap(),
            std::path::Path::new("missing")
        );
    }

    #[test]
    fn simultaneous_renames_have_one_winner_without_losing_the_other_source() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let target = dir.path().join("target");
        fs::write(&a, "A").unwrap();
        fs::write(&b, "B").unwrap();
        let barrier = Barrier::new(2);
        let run = |source: &std::path::Path| {
            barrier.wait();
            super::rename(source, &target)
        };
        let results = thread::scope(|scope| {
            let first = scope.spawn(|| run(&a));
            let second = scope.spawn(|| run(&b));
            (first.join().unwrap(), second.join().unwrap())
        });
        assert_ne!(results.0.is_ok(), results.1.is_ok());
        let (remaining, winner, contents) = if results.0.is_ok() {
            (&b, "A", "B")
        } else {
            (&a, "B", "A")
        };
        assert_eq!(fs::read_to_string(target).unwrap(), winner);
        assert_eq!(fs::read_to_string(remaining).unwrap(), contents);
    }
}
