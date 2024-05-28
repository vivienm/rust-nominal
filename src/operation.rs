use std::{fmt, fs, path::Path};

use crate::{
    error::ApplyError,
    fsutil::{common_ancestor, path_exists},
};

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
        ls_colors: &lscolors::LsColors,
        w: &mut W,
    ) -> std::io::Result<()>
    where
        W: std::io::Write,
    {
        let source = self.source.as_ref();
        let target = self.target.as_ref();

        let source_style = style_for_path(ls_colors, source);
        let target_style = style_for_path(ls_colors, target);

        match common_ancestor(source, target) {
            Some(common) => {
                let common_style = style_for_path(ls_colors, common);
                let source = source.strip_prefix(common).unwrap();
                let target = target.strip_prefix(common).unwrap();

                writeln!(
                    w,
                    "{}{}/{}{{{}{}{} => {}{}{}}}",
                    common_style.prefix(),
                    common.display(),
                    common_style.suffix(),
                    source_style.prefix(),
                    source.display(),
                    source_style.suffix(),
                    target_style.prefix(),
                    target.display(),
                    target_style.suffix()
                )?;
            }
            None => {
                writeln!(
                    w,
                    "{}{}{} => {}{}{}",
                    source_style.prefix(),
                    source.display(),
                    source_style.suffix(),
                    target_style.prefix(),
                    target.display(),
                    target_style.suffix()
                )?;
            }
        }
        Ok(())
    }

    /// Executes the rename operation.
    pub fn apply(&self) -> Result<(), ApplyError> {
        let source = self.source.as_ref();
        let target = self.target.as_ref();

        // We check before renaming to avoid overwriting the target.
        if path_exists(target).map_err(|err| ApplyError::from_io(source, target, err))? {
            return Err(ApplyError::target_exists(source, target));
        }

        if let Some(target_parent) = target.parent() {
            if !target_parent.exists() {
                tracing::debug!("creating parent directory for {}", target.display());
                fs::create_dir_all(target_parent)
                    .map_err(|err| ApplyError::from_io(source, target, err))?;
            }
        }
        tracing::debug!("renaming {} to {}", source.display(), target.display());
        fs::rename(source, target).map_err(|err| ApplyError::from_io(source, target, err))?;
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
        match common_ancestor(source, target) {
            Some(common) => {
                let source = source.strip_prefix(common).unwrap();
                let target = target.strip_prefix(common).unwrap();
                write!(
                    f,
                    "{}/{{{} => {}}}",
                    common.display(),
                    source.display(),
                    target.display()
                )
            }
            None => write!(f, "{} => {}", source.display(), target.display()),
        }
    }
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
