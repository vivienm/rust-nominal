use std::{fs, io, path::Path};

/// Returns the common ancestor of two paths.
pub fn common_ancestor<'a>(path_1: &'a Path, path_2: &'a Path) -> Option<&'a Path> {
    path_1
        .ancestors()
        .find(|&ancestor| !ancestor.as_os_str().is_empty() && path_2.starts_with(ancestor))
}

/// Whether a target is occupied by an entry other than the source entry.
/// Inspect the directory entries themselves, including dangling symlinks.
pub(crate) fn target_conflicts(source: &Path, target: &Path) -> io::Result<bool> {
    let target_metadata = match fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    let source_metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        // A missing source must never hide an occupied target.
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(err),
    };
    Ok(!same_entry(
        source,
        target,
        &source_metadata,
        &target_metadata,
    )?)
}

#[cfg(unix)]
fn same_entry(
    source: &Path,
    target: &Path,
    source_metadata: &fs::Metadata,
    target_metadata: &fs::Metadata,
) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    if source_metadata.dev() != target_metadata.dev()
        || source_metadata.ino() != target_metadata.ino()
    {
        return Ok(false);
    }
    // Separate hard links share an inode, but rename between them is a no-op.
    // Conservatively reject existing targets for multiply linked files, even
    // for a case-only spelling change. Directories cannot have user hard links.
    if !source_metadata.is_dir() && source_metadata.nlink() != 1 {
        return Ok(false);
    }
    // A case-only change must remain within the same directory. Canonicalize
    // only the parents: canonicalizing the entries would follow symlinks.
    let parent = |path: &Path| {
        fs::canonicalize(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
    };
    Ok(parent(source)? == parent(target)?)
}

#[cfg(not(unix))]
fn same_entry(
    source: &Path,
    target: &Path,
    source_metadata: &fs::Metadata,
    target_metadata: &fs::Metadata,
) -> io::Result<bool> {
    // Canonicalization on Windows resolves the stored spelling, but follows
    // symlinks. Reject symlink destinations rather than compare their referents.
    if source_metadata.is_symlink() || target_metadata.is_symlink() {
        return Ok(false);
    }
    Ok(fs::canonicalize(source)? == fs::canonicalize(target)?)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn common_ancestor() {
        let path_1 = Path::new("/a/b/c/d");
        let path_2 = Path::new("/a/b/e/f");
        assert_eq!(
            super::common_ancestor(path_1, path_2),
            Some(Path::new("/a/b"))
        );

        let path_1 = Path::new("/a/b/c/d");
        let path_2 = Path::new("/a/b/c/d/e/f");
        assert_eq!(
            super::common_ancestor(path_1, path_2),
            Some(Path::new("/a/b/c/d"))
        );

        let path_1 = Path::new("/a/b/c/d");
        let path_2 = Path::new("/x/y/z");
        assert_eq!(super::common_ancestor(path_1, path_2), Some(Path::new("/")));

        let path_1 = Path::new("a/b/c/d");
        let path_2 = Path::new("x/y/z");
        assert_eq!(super::common_ancestor(path_1, path_2), None);
    }
}
