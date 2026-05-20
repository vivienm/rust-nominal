use std::{fmt, fs, io, path::Path};

use crate::{error::RenameError, fsutil::common_ancestor};

/// A rename operation.
#[derive(Debug)]
pub struct Rename<S, T> {
    pub source: S,
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
    pub fn write_to<W>(&self, writer: &mut W) -> std::io::Result<()>
    where
        W: std::io::Write,
    {
        writeln!(writer, "{}", self)
    }

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
    /// The target is checked for existence before renaming to avoid
    /// overwriting it. This check and the rename itself are not atomic:
    /// a concurrent process creating the target between the two calls
    /// can still be overwritten.
    pub fn apply(&self) -> Result<(), RenameError> {
        let source = self.source.as_ref();
        let target = self.target.as_ref();

        // Reject targets that exist and refer to a different file. Targets
        // that resolve to the source itself (e.g. a case-only rename on a
        // case-insensitive filesystem) are allowed through.
        match same_file::is_same_file(source, target) {
            Ok(true) => {}
            Ok(false) => return Err(RenameError::TargetExists),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }

        if let Some(target_parent) = target.parent()
            && !target_parent.exists()
        {
            tracing::debug!("creating parent directory for {}", target.display());
            fs::create_dir_all(target_parent)?;
        }
        tracing::debug!("renaming {} to {}", source.display(), target.display());
        fs::rename(source, target)?;
        Ok(())
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
    use super::Rename;

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
