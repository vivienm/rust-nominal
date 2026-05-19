use std::{io, path::Path};

use crate::{error::ApplyError, operation::Rename};

/// A renaming plan.
#[derive(Debug)]
#[must_use]
pub struct Plan<S, T> {
    pub(crate) renames: Vec<Rename<S, T>>,
}

impl<S, T> Plan<S, T> {
    /// Returns `true` if the plan is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// # use nominal::{Plan, Renamer};
    /// let plan: Plan<&str, &str> = Renamer::new().plan()?;
    /// assert!(plan.is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn is_empty(&self) -> bool {
        self.renames.is_empty()
    }

    /// Returns the number of rename operations in the plan.
    ///
    /// # Examples
    ///
    /// ```
    /// # use nominal::{Plan, Renamer};
    /// let mut renamer = Renamer::new();
    /// renamer.add("old.txt", "new.txt");
    ///
    /// let plan = renamer.plan()?;
    /// assert_eq!(plan.len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn len(&self) -> usize {
        self.renames.len()
    }
}

impl<S, T> Plan<S, T>
where
    S: AsRef<Path>,
    T: AsRef<Path>,
{
    /// Writes the plan to the specified writer.
    pub fn write_to<W>(&self, writer: &mut W) -> io::Result<()>
    where
        W: io::Write,
    {
        for rename in &self.renames {
            rename.write_to(writer)?;
        }
        Ok(())
    }

    /// Writes the plan to the specified writer, with ANSI colors.
    ///
    /// To color paths using the `LS_COLORS` environment variable:
    ///
    /// ```ignore
    /// let ls_colors = lscolors::LsColors::from_env().unwrap_or_default();
    /// plan.write_colored_to(&mut std::io::stdout(), &ls_colors)?;
    /// ```
    #[cfg(feature = "ansi")]
    pub fn write_colored_to<W>(
        &self,
        writer: &mut W,
        ls_colors: &lscolors::LsColors,
    ) -> io::Result<()>
    where
        W: io::Write,
    {
        for rename in &self.renames {
            rename.write_colored_to(writer, ls_colors)?;
        }
        Ok(())
    }

    /// Prompts the user to confirm the plan.
    ///
    /// If the plan is empty, this returns [`None`]. Otherwise, it prompts the
    /// user to confirm the plan and returns the user's response.
    ///
    /// # Examples
    ///
    /// ```
    /// # use nominal::{Plan, Renamer};
    /// let plan: Plan<&str, &str> = Renamer::new().plan()?;
    /// assert!(plan.confirm()?.is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[cfg(feature = "confirm")]
    pub fn confirm(&self) -> io::Result<Option<bool>> {
        Ok(if self.is_empty() {
            None
        } else {
            let prompt = dialoguer::Confirm::new()
                .with_prompt("Proceed?")
                .interact()
                .map_err(|dialoguer::Error::IO(err)| err)?;
            Some(prompt)
        })
    }

    /// Executes the plan.
    ///
    /// Each rename checks that its target does not exist before proceeding,
    /// but this check is not atomic with the rename itself: a concurrent
    /// process creating the target in between can still be overwritten.
    ///
    /// If a rename fails partway through, the operations applied so far are
    /// not rolled back. The number of operations that succeeded is reported
    /// in [`ApplyError::applied`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::fs::File;
    /// # use nominal::{Plan, Renamer};
    /// let temp_dir = tempfile::tempdir()?;
    /// let old_path = temp_dir.path().join("old.txt");
    /// let new_path = temp_dir.path().join("new.txt");
    ///
    /// File::create(&old_path)?;
    ///
    /// let mut renamer = Renamer::new();
    /// renamer.add(&old_path, &new_path);
    ///
    /// let plan = renamer.plan()?;
    /// plan.apply()?;
    ///
    /// assert!(!old_path.exists());
    /// assert!(new_path.exists());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn apply(self) -> Result<(), ApplyError> {
        for (applied, rename) in self.renames.iter().enumerate() {
            rename.apply().map_err(|mut err| {
                err.applied = applied;
                err
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use crate::Renamer;

    #[test]
    fn apply_reports_partial_count_on_failure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        // First rename succeeds, second collides with a pre-existing target.
        File::create(dir.join("a")).unwrap();
        File::create(dir.join("c")).unwrap();
        File::create(dir.join("d")).unwrap();

        let mut renamer = Renamer::new();
        renamer.add(dir.join("a"), dir.join("b"));
        renamer.add(dir.join("c"), dir.join("d"));

        let err = renamer
            .plan()
            .unwrap()
            .apply()
            .expect_err("second rename should fail");
        assert_eq!(err.applied, 1);
        assert_eq!(err.source, dir.join("c"));
        assert_eq!(err.target, dir.join("d"));
    }
}
