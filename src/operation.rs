use std::{fmt, fs, path::Path};

use crate::{
    error::RenameError,
    fsutil::{ResolvedPath, TargetState, common_ancestor, ends_with_entry_name, target_state},
};

/// A rename operation.
#[derive(Debug)]
pub struct Rename<S, T> {
    /// The path to rename.
    pub source: S,
    /// The destination path.
    pub target: T,
}

impl<S, T> Rename<S, T> {
    /// Creates a new rename operation.
    pub fn new(source: S, target: T) -> Self {
        Self { source, target }
    }
}

impl<S, T> Rename<S, T>
where
    S: AsRef<Path>,
    T: AsRef<Path>,
{
    /// Writes the rename operation to the specified writer.
    pub fn write_to<W>(&self, writer: &mut W) -> std::io::Result<()>
    where
        W: std::io::Write,
    {
        writeln!(writer, "{}", self)
    }

    /// Writes the rename operation to the specified writer, with ANSI colors.
    #[cfg(feature = "ansi")]
    pub fn write_colored_to<W>(
        &self,
        writer: &mut W,
        ls_colors: &lscolors::LsColors,
    ) -> std::io::Result<()>
    where
        W: std::io::Write,
    {
        let source = self.source.as_ref();
        let target = self.target.as_ref();

        let source_style = style_for_path(ls_colors, source);
        let target_style = style_for_path(ls_colors, target);

        let (common, source_rest, target_rest) = split_for_display(source, target);
        match common {
            Some(common) => {
                let common_style = style_for_path(ls_colors, common);
                writeln!(
                    writer,
                    "{}{}/{}{{{}{}{} => {}{}{}}}",
                    common_style.prefix(),
                    common.display(),
                    common_style.suffix(),
                    source_style.prefix(),
                    source_rest.display(),
                    source_style.suffix(),
                    target_style.prefix(),
                    target_rest.display(),
                    target_style.suffix()
                )
            }
            None => writeln!(
                writer,
                "{}{}{} => {}{}{}",
                source_style.prefix(),
                source_rest.display(),
                source_style.suffix(),
                target_style.prefix(),
                target_rest.display(),
                target_style.suffix()
            ),
        }
    }

    /// Executes the rename operation.
    ///
    /// Source and target paths are unrestricted. Callers must enforce any
    /// required directory confinement before passing paths to this method.
    ///
    /// Parent aliases are resolved before checking the target or creating
    /// directories, as in [`crate::Renamer::prepare`]. A `..` component after a
    /// missing directory is rejected because it cannot be resolved on disk.
    ///
    /// The target is checked for existence before renaming to avoid
    /// overwriting it, using the same conflict rules as [`crate::Plan::reject_conflicts`].
    /// The system rename also refuses replacement atomically, including if a
    /// target appears after the check. Linux, Android, Apple platforms and
    /// Windows are supported; other platforms or filesystems without this
    /// primitive return an I/O error. There is no overwriting fallback.
    ///
    /// When source and target are spelling variants of the same entry, the
    /// rename uses a temporary name beside the source. This ensures the
    /// stored spelling changes even on filesystems where a direct rename is
    /// a no-op. The entry is briefly absent from its original path. If the
    /// final move fails, restoration to the source path is attempted. If that
    /// also fails, [`RenameError::RecoveryFailed`] reports where the entry was
    /// retained; it is never deleted by temporary-directory cleanup.
    pub fn apply(&self) -> Result<(), RenameError> {
        let source = ResolvedPath::new(&self.source)?;
        let target = ResolvedPath::new(&self.target)?;
        apply_resolved(&source, &target)
    }
}

/// Executes captured paths without resolving their parents again.
/// Resolution does not pin directory entries: recheck destination occupancy
/// against the current filesystem and refuse replacement at each rename.
pub(crate) fn apply_resolved(
    source: &ResolvedPath,
    target: &ResolvedPath,
) -> Result<(), RenameError> {
    let (source, target) = (source.as_path(), target.as_path());

    match target_state(source, target)? {
        TargetState::Conflict => return Err(RenameError::TargetExists),
        TargetState::SameEntry
            if source.file_name().is_some() && source.file_name() != target.file_name() =>
        {
            return rename_via_temporary(source, target, rename_to_free_target);
        }
        TargetState::SameEntry if ends_with_entry_name(source) && ends_with_entry_name(target) => {
            // Identical entry names (possibly through parent aliases) need
            // no system rename. Keep suffix constraints out of this case.
            return Ok(());
        }
        _ => {}
    }

    if let Some(target_parent) = target.parent()
        && !target_parent.exists()
    {
        tracing::debug!("creating parent directory for {}", target.display());
        fs::create_dir_all(target_parent)?;
    }
    tracing::debug!("renaming {} to {}", source.display(), target.display());
    crate::noreplace::rename(source, target)
}

/// Each stage rechecks occupancy, including when restoring the source.
fn rename_to_free_target(source: &Path, target: &Path) -> Result<(), RenameError> {
    // Each stage must actually vacate its source, so even an alias appearing
    // at the destination is a conflict here rather than a successful no-op.
    if target_state(source, target)? != TargetState::Missing {
        return Err(RenameError::TargetExists);
    }
    tracing::debug!("renaming {} to {}", source.display(), target.display());
    crate::noreplace::rename(source, target)
}

fn rename_via_temporary(
    source: &Path,
    target: &Path,
    mut rename: impl FnMut(&Path, &Path) -> Result<(), RenameError>,
) -> Result<(), RenameError> {
    let parent = source.parent().expect("resolved entry has a parent");
    // Reserve a unique sibling name, then release the empty placeholder before
    // moving anything. Keep it out of TempDir's recursive cleanup: a failed
    // recovery must leave user data at the reported temporary path.
    // A sibling also preserves relative symlink meaning and avoids requiring
    // write permission on a moved directory just to update its `..` entry.
    let temporary = tempfile::Builder::new()
        .prefix(".nominal-")
        .tempdir_in(parent)?
        .keep();
    fs::remove_dir(&temporary)?;
    rename(source, &temporary)?;
    match rename(&temporary, target) {
        Ok(()) => Ok(()),
        Err(error) => match rename(&temporary, source) {
            Ok(()) => Err(error),
            Err(recovery_error) => Err(RenameError::RecoveryFailed {
                temporary_path: temporary,
                source: Box::new(error),
                recovery_error: Box::new(recovery_error),
            }),
        },
    }
}

impl<S, T> From<(S, T)> for Rename<S, T> {
    fn from((source, target): (S, T)) -> Self {
        Self::new(source, target)
    }
}

impl<S, T> From<Rename<S, T>> for (S, T) {
    fn from(rename: Rename<S, T>) -> Self {
        (rename.source, rename.target)
    }
}

impl<S, T> fmt::Display for Rename<S, T>
where
    S: AsRef<Path>,
    T: AsRef<Path>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let source = self.source.as_ref();
        let target = self.target.as_ref();
        let (common, source_rest, target_rest) = split_for_display(source, target);
        match common {
            Some(common) => write!(
                f,
                "{}/{{{} => {}}}",
                common.display(),
                source_rest.display(),
                target_rest.display()
            ),
            None => write!(f, "{} => {}", source_rest.display(), target_rest.display()),
        }
    }
}

/// Splits two paths for a `common/{a => b}` style display, returning the
/// common prefix (if any) and the two remainders to print.
///
/// When factoring a common prefix wouldn't actually shorten the output, the
/// first element is `None` and the remainders are the original paths. This
/// happens when the only shared ancestor is the filesystem root (`/` on Unix,
/// `C:\` on Windows), or when one path is a prefix of the other.
fn split_for_display<'a>(
    source: &'a Path,
    target: &'a Path,
) -> (Option<&'a Path>, &'a Path, &'a Path) {
    let Some(common) = common_ancestor(source, target) else {
        return (None, source, target);
    };
    // Skip a root-only common ancestor (`/` on Unix, `C:\` on Windows): factoring
    // it out would produce `/{a => b}`, which reads worse than `/a => /b`.
    if common.parent().is_none() {
        return (None, source, target);
    }
    let (Ok(source_rest), Ok(target_rest)) =
        (source.strip_prefix(common), target.strip_prefix(common))
    else {
        return (None, source, target);
    };
    if source_rest.as_os_str().is_empty() || target_rest.as_os_str().is_empty() {
        return (None, source, target);
    }
    (Some(common), source_rest, target_rest)
}

#[cfg(feature = "ansi")]
fn style_for_path<P>(ls_colors: &lscolors::LsColors, path: P) -> nu_ansi_term::Style
where
    P: AsRef<Path>,
{
    use lscolors::Style;

    ls_colors
        .style_for_path(path)
        .map(Style::to_nu_ansi_term_style)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::{fs, io};

    use super::{Rename, rename_to_free_target, rename_via_temporary};
    use crate::RenameError;

    #[test]
    fn failed_staging_leaves_source_and_removes_temporary_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, "original").unwrap();
        let result = rename_via_temporary(&source, &target, |_, _| {
            Err(io::Error::from(io::ErrorKind::PermissionDenied).into())
        });
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&source).unwrap(), "original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_final_move_restores_source_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, "original").unwrap();
        let mut calls = 0;
        let result = rename_via_temporary(&source, &target, |from, to| {
            calls += 1;
            if calls == 2 {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied).into());
            }
            rename_to_free_target(from, to)
        });
        assert!(
            matches!(result, Err(RenameError::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied)
        );
        assert_eq!(fs::read_to_string(&source).unwrap(), "original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn target_created_during_staging_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, "original").unwrap();
        let mut calls = 0;
        let result = rename_via_temporary(&source, &target, |from, to| {
            calls += 1;
            if calls == 2 {
                fs::write(&target, "new target").unwrap();
            }
            rename_to_free_target(from, to)
        });
        assert!(matches!(result, Err(RenameError::TargetExists)));
        assert_eq!(fs::read_to_string(&source).unwrap(), "original");
        assert_eq!(fs::read_to_string(&target).unwrap(), "new target");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn failed_recovery_retains_data_and_reports_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("contents"), "original").unwrap();
        let mut calls = 0;
        let error = rename_via_temporary(&source, &target, |from, to| {
            calls += 1;
            if calls == 2 {
                fs::write(&target, "new target").unwrap();
                fs::write(&source, "new source").unwrap();
            }
            rename_to_free_target(from, to)
        })
        .unwrap_err();
        let RenameError::RecoveryFailed {
            temporary_path,
            source: cause,
            recovery_error,
        } = error
        else {
            panic!("expected a recovery error: {error:?}");
        };
        assert!(matches!(*cause, RenameError::TargetExists));
        assert!(matches!(*recovery_error, RenameError::TargetExists));
        assert!(temporary_path.is_absolute());
        assert_eq!(
            fs::read_to_string(temporary_path.join("contents")).unwrap(),
            "original"
        );
        assert_eq!(fs::read_to_string(&source).unwrap(), "new source");
        assert_eq!(fs::read_to_string(&target).unwrap(), "new target");
    }

    #[test]
    fn display_factors_non_trivial_common_prefix() {
        let r = Rename::new("/a/b/c", "/a/b/d");
        assert_eq!(r.to_string(), "/a/b/{c => d}");
    }

    #[test]
    fn display_does_not_factor_root_only_ancestor() {
        let r = Rename::new("/a/b", "/x/y");
        assert_eq!(r.to_string(), "/a/b => /x/y");
    }

    #[test]
    fn display_does_not_factor_when_one_path_is_a_prefix_of_the_other() {
        let r = Rename::new("a", "a/b");
        assert_eq!(r.to_string(), "a => a/b");

        let r = Rename::new("a/b", "a");
        assert_eq!(r.to_string(), "a/b => a");
    }

    #[test]
    fn display_falls_back_when_no_common_ancestor() {
        let r = Rename::new("a/b", "x/y");
        assert_eq!(r.to_string(), "a/b => x/y");
    }
}
