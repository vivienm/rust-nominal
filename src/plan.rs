use std::{io, path::Path};

use crate::{error::ApplyError, operation::Rename};

/// A renaming plan.
#[derive(Debug)]
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

    #[cfg(feature = "ansi")]
    /// Writes the plan to the specified writer, with ANSI colors.
    pub fn write_colored_to<W>(&self, writer: &mut W) -> io::Result<()>
    where
        W: io::Write,
    {
        let ls_colors = lscolors::LsColors::from_env().unwrap_or_else(|| {
            tracing::warn!("could not read LS_COLORS environment variable");
            lscolors::LsColors::default()
        });
        for rename in &self.renames {
            rename.write_colored_to(&ls_colors, writer)?;
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
        // TODO: Multiple rounds to handle acyclic conflicts.
        for rename in &self.renames {
            rename.apply()?;
        }
        Ok(())
    }
}
