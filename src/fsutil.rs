use std::{io, path::Path};

/// Returns the common ancestor of two paths.
pub fn common_ancestor<'a>(path_1: &'a Path, path_2: &'a Path) -> Option<&'a Path> {
    path_1
        .ancestors()
        .find(|&ancestor| !ancestor.as_os_str().is_empty() && path_2.starts_with(ancestor))
}

/// Tests whether a path exists.
///
/// This function does not follow symbolic links.
pub fn path_exists<P>(path: P) -> io::Result<bool>
where
    P: AsRef<Path>,
{
    let path = path.as_ref();
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io, path::Path};

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

    #[test]
    fn path_exists() -> io::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let file_path = temp_dir.path().join("file.txt");
        let link_path = temp_dir.path().join("link.txt");

        // The file does not exist yet.
        assert!(!super::path_exists(&file_path)?);

        // Create the file, check that it exists.
        fs::File::create(&file_path)?;
        assert!(super::path_exists(&file_path)?);

        // Create a symbolic link, check that it exists.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&file_path, &link_path)?;
            assert!(super::path_exists(&link_path)?);
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(&file_path, &link_path)?;
        }
        assert!(super::path_exists(&link_path)?);

        // Remove the file, check that the symbolic link still exists.
        fs::remove_file(&file_path)?;
        assert!(super::path_exists(&link_path)?);

        Ok(())
    }
}
