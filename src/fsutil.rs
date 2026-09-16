use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Returns the common ancestor of two paths.
pub fn common_ancestor<'a>(path_1: &'a Path, path_2: &'a Path) -> Option<&'a Path> {
    path_1
        .ancestors()
        .find(|&ancestor| !ancestor.as_os_str().is_empty() && path_2.starts_with(ancestor))
}

/// Whether the raw path ends directly with its entry name.
/// `file_name()` ignores trailing separators and `.` components, so compare
/// against the raw spelling: `dir` passes, but `dir/` and `dir/.` do not.
/// This prevents same-entry no-op detection from bypassing the filesystem
/// checks required by those suffixes.
pub(crate) fn ends_with_entry_name(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        path.as_os_str()
            .as_encoded_bytes()
            .ends_with(name.as_encoded_bytes())
    })
}

/// The parent spelling used for resolution and its preparation-local cache.
pub(crate) fn entry_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// A path whose parents have been resolved, preserving the final entry spelling.
/// This captures a spelling, not an open handle: directories can still change
/// before execution, and filesystem operations will observe their current state.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedPath(PathBuf);

impl ResolvedPath {
    /// Resolves parent aliases by inspecting the filesystem, without following
    /// the final directory entry. Missing parent directories are permitted so
    /// apply can create them later; trailing separators and `.` are preserved.
    pub(crate) fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::with_parent_resolver(path.as_ref(), resolve_directory)
    }

    /// Use the same endpoint rules with a preparation-local parent resolver.
    pub(crate) fn with_parent_resolver<P: AsRef<Path>>(
        path: &Path,
        resolve_parent: impl FnOnce(&Path) -> io::Result<P>,
    ) -> io::Result<Self> {
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "path must name a directory entry",
            )
        })?;
        let parent = entry_parent(path);
        let mut resolved = resolve_parent(parent)?.as_ref().join(name);
        // Path components omit trailing separators and `.`. Preserve these in the
        // executed path: `file/` must not silently become a valid rename of `file`.
        let raw = path.as_os_str().as_encoded_bytes();
        let final_component = raw
            .rsplit(|&byte| byte == b'/' || (cfg!(windows) && byte == b'\\'))
            .find(|component| !component.is_empty());
        if final_component == Some(b".") {
            resolved.push(".");
        } else if raw.ends_with(b"/") || (cfg!(windows) && raw.ends_with(b"\\")) {
            resolved.push("");
        }
        Ok(Self(resolved))
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for ResolvedPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl From<ResolvedPath> for PathBuf {
    fn from(path: ResolvedPath) -> Self {
        path.0
    }
}

pub(crate) fn resolve_directory(path: &Path) -> io::Result<PathBuf> {
    let mut unresolved = Vec::new();
    let mut current = path;
    let mut resolved = loop {
        match fs::canonicalize(current) {
            Ok(resolved) => {
                if !fs::metadata(&resolved)?.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        "parent is not a directory",
                    ));
                }
                break resolved;
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                // A dangling symlink is not a missing directory we can create.
                match fs::symlink_metadata(current) {
                    Err(missing) if missing.kind() == io::ErrorKind::NotFound => {}
                    Err(other) => return Err(other),
                    Ok(_) => return Err(err),
                }
                // Do not simplify `missing/..`: it does not resolve on disk.
                let Some(name) = current.file_name() else {
                    return Err(err);
                };
                unresolved.push(name);
                current = current
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
            }
            Err(err) => return Err(err),
        }
    };
    for name in unresolved.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

/// A resolved execution path paired with its preparation-time entry identity.
/// Private fields prevent constructing mismatched paths and keys.
#[derive(Debug, Clone)]
pub(crate) struct IdentifiedPath {
    resolved: ResolvedPath,
    key: EntryKey,
}

impl IdentifiedPath {
    pub(crate) fn new(mut resolved: ResolvedPath) -> Result<Self, IdentificationError> {
        match entry_key(resolved.as_path()) {
            Ok(key) => {
                resolved.0.shrink_to_fit();
                Ok(Self { resolved, key })
            }
            Err(source) => Err(IdentificationError { resolved, source }),
        }
    }

    pub(crate) fn as_path(&self) -> &Path {
        self.resolved.as_path()
    }

    pub(crate) fn key(&self) -> &EntryKey {
        &self.key
    }

    pub(crate) fn resolved(&self) -> &ResolvedPath {
        &self.resolved
    }

    pub(crate) fn into_resolved(self) -> ResolvedPath {
        self.resolved
    }
}

impl AsRef<Path> for IdentifiedPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

/// An identification failure retaining the resolved path for overlap checks.
#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub(crate) struct IdentificationError {
    pub(crate) resolved: ResolvedPath,
    #[source]
    pub(crate) source: io::Error,
}

/// The on-disk anchor and unresolved suffix of a directory entry.
/// Keep this separate from execution paths, which retain the requested spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct EntryKey {
    anchor: EntryAnchor,
    suffix: PathBuf,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EntryAnchor {
    dev: u64,
    ino: u64,
}

#[cfg(not(unix))]
type EntryAnchor = PathBuf;

#[cfg(unix)]
fn entry_anchor(_path: &Path, metadata: &fs::Metadata) -> io::Result<Option<EntryAnchor>> {
    use std::os::unix::fs::MetadataExt;

    // Hard links are separate directory entries. Use their parent and name
    // instead of their shared inode, matching same_entry's conservative rule.
    Ok(
        (metadata.is_dir() || metadata.nlink() == 1).then(|| EntryAnchor {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }),
    )
}

#[cfg(not(unix))]
fn entry_anchor(path: &Path, metadata: &fs::Metadata) -> io::Result<Option<EntryAnchor>> {
    // Windows canonicalization resolves stored case, but follows symlinks.
    // Keep symlink names distinct, as in same_entry on these platforms.
    if metadata.is_symlink() {
        Ok(None)
    } else {
        fs::canonicalize(path).map(Some)
    }
}

/// Identify existing case aliases without opening files or following the final
/// symlink. Missing suffixes retain their spelling under an existing anchor.
pub(crate) fn entry_key(path: &Path) -> io::Result<EntryKey> {
    // Suffix constraints affect execution, not the dependency graph.
    let normalized: PathBuf = path.components().collect();
    let mut current = normalized.as_path();
    let mut suffix = Vec::new();
    let anchor = loop {
        match fs::symlink_metadata(current) {
            Ok(metadata) => {
                if let Some(anchor) = entry_anchor(current, &metadata)? {
                    break anchor;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        let name = current.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "path has no existing anchor")
        })?;
        suffix.push(name);
        current = current.parent().expect("entry has a parent");
    };
    Ok(EntryKey {
        anchor,
        suffix: suffix.into_iter().rev().collect(),
    })
}

/// Whether a target is occupied by an entry other than the source entry.
/// Inspect the directory entries themselves, including dangling symlinks.
pub(crate) fn target_conflicts(source: &Path, target: &Path) -> io::Result<bool> {
    Ok(matches!(
        target_state(source, target)?,
        TargetState::Conflict
    ))
}

/// Occupancy of a target, distinguishing case aliases from free destinations.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TargetState {
    Missing,
    SameEntry,
    Conflict,
}

pub(crate) fn target_state(source: &Path, target: &Path) -> io::Result<TargetState> {
    let target_metadata = match fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(TargetState::Missing),
        Err(err) => return Err(err),
    };
    let source_metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        // A missing source must never hide an occupied target.
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(TargetState::Conflict),
        Err(err) => return Err(err),
    };
    Ok(
        if same_entry(source, target, &source_metadata, &target_metadata)? {
            TargetState::SameEntry
        } else {
            TargetState::Conflict
        },
    )
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
    // A case-only change must remain within the same directory. Compare parent
    // identities too: canonicalize need not resolve stored case on Unix.
    let parent = |path: &Path| {
        fs::metadata(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
    };
    let source_parent = parent(source)?;
    let target_parent = parent(target)?;
    Ok(source_parent.dev() == target_parent.dev() && source_parent.ino() == target_parent.ino())
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
