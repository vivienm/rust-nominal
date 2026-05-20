use std::{collections::HashSet, io, path::Path, vec};

use crate::{
    error::{ApplyError, FsConflict},
    operation::Rename,
};

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
    /// # Errors
    ///
    /// Returns an [`io::Error`] if the prompt cannot be displayed or read.
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

    /// Inspects the filesystem and returns the renames whose target path
    /// already refers to a different file on disk.
    ///
    /// Conflicting renames are removed from the plan, so a subsequent
    /// [`apply`](Self::apply) only attempts the entries that are still safe
    /// to perform. Two paths that resolve to the same filesystem entry (e.g.
    /// a case-only rename on a case-insensitive filesystem, or a symlink to
    /// the source) are not treated as conflicts.
    ///
    /// Targets shared by multiple renames in the same batch are already
    /// rejected at [`plan`](crate::Renamer::plan) time as
    /// [`PlanError::DuplicateTarget`](crate::PlanError::DuplicateTarget); this
    /// method only looks for collisions with files outside the batch.
    ///
    /// The check is not atomic with the rename itself: a target that is
    /// absent here may appear before [`apply`](Self::apply) runs.
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] if a path's existence cannot be determined.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::fs::File;
    /// # use nominal::Renamer;
    /// let temp_dir = tempfile::tempdir()?;
    /// let dir = temp_dir.path();
    /// File::create(dir.join("a"))?;
    /// File::create(dir.join("c"))?;
    /// File::create(dir.join("b"))?; // pre-existing target
    ///
    /// let mut renamer = Renamer::new();
    /// renamer.add(dir.join("a"), dir.join("b"));
    /// renamer.add(dir.join("c"), dir.join("d"));
    ///
    /// let mut plan = renamer.plan()?;
    /// let conflicts = plan.check_fs()?;
    /// assert_eq!(conflicts.len(), 1);
    /// assert_eq!(plan.len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn check_fs(&mut self) -> io::Result<Vec<FsConflict>> {
        let sources: HashSet<&Path> = self.renames.iter().map(|r| r.source.as_ref()).collect();

        // Mark conflicts in a first pass so the immutable borrow on
        // `self.renames` is released before we start moving entries.
        let mut is_conflict = Vec::with_capacity(self.renames.len());
        for rename in &self.renames {
            let source = rename.source.as_ref();
            let target = rename.target.as_ref();
            // A target that is itself a source within the batch will be
            // vacated by another rename (the topological sort guarantees the
            // order), so it is not a conflict.
            let conflict = if sources.contains(target) {
                false
            } else {
                match same_file::is_same_file(source, target) {
                    Ok(same) => !same,
                    Err(err) if err.kind() == io::ErrorKind::NotFound => false,
                    Err(err) => return Err(err),
                }
            };
            is_conflict.push(conflict);
        }
        drop(sources);

        let mut conflicts = Vec::new();
        self.renames = std::mem::take(&mut self.renames)
            .into_iter()
            .zip(is_conflict)
            .filter_map(|(rename, conflict)| {
                if conflict {
                    conflicts.push(FsConflict::TargetExists {
                        target_path: rename.target.as_ref().to_path_buf(),
                    });
                    None
                } else {
                    Some(rename)
                }
            })
            .collect();
        Ok(conflicts)
    }

    /// Executes the plan, stopping at the first failure.
    ///
    /// Each rename checks that its target does not exist before proceeding,
    /// but this check is not atomic with the rename itself: a concurrent
    /// process creating the target in between can still be overwritten.
    ///
    /// If a rename fails partway through, the operations applied so far are
    /// not rolled back. The number of operations that succeeded is reported
    /// in [`ApplyError::applied`].
    ///
    /// To continue past failures, use [`apply_iter`](Self::apply_iter)
    /// instead.
    ///
    /// # Errors
    ///
    /// Returns an [`ApplyError`] if any rename fails. Renames already
    /// applied are not rolled back.
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
        for result in self.apply_iter() {
            result?;
        }
        Ok(())
    }

    /// Executes the plan one rename at a time, yielding the outcome of each.
    ///
    /// The iterator runs each rename as it is pulled and yields either the
    /// completed [`Rename`] or an [`ApplyError`] describing the failure.
    /// Iteration continues after errors, so callers can choose to keep going
    /// (best-effort) or stop early. The TOCTOU and partial-application
    /// caveats from [`apply`](Self::apply) apply to each step.
    ///
    /// # Examples
    ///
    /// Best-effort: rename what we can, collect failures.
    ///
    /// ```
    /// # use std::fs::File;
    /// # use nominal::Renamer;
    /// let temp_dir = tempfile::tempdir()?;
    /// let dir = temp_dir.path();
    /// File::create(dir.join("a"))?;
    /// File::create(dir.join("c"))?;
    /// File::create(dir.join("d"))?; // pre-existing target, will collide
    ///
    /// let mut renamer = Renamer::new();
    /// renamer.add(dir.join("a"), dir.join("b"));
    /// renamer.add(dir.join("c"), dir.join("d"));
    ///
    /// let mut renamed = 0;
    /// let mut errors = Vec::new();
    /// for result in renamer.plan()?.apply_iter() {
    ///     match result {
    ///         Ok(_) => renamed += 1,
    ///         Err(err) => errors.push(err),
    ///     }
    /// }
    /// assert_eq!(renamed, 1);
    /// assert_eq!(errors.len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn apply_iter(self) -> ApplyIter<S, T> {
        ApplyIter {
            iter: self.renames.into_iter().enumerate(),
        }
    }
}

/// Iterator returned by [`Plan::apply_iter`].
///
/// Each call to [`next`](Iterator::next) executes one rename and yields its
/// outcome. Created by [`Plan::apply_iter`]; see that method for details.
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct ApplyIter<S, T> {
    iter: std::iter::Enumerate<vec::IntoIter<Rename<S, T>>>,
}

impl<S, T> Iterator for ApplyIter<S, T>
where
    S: AsRef<Path>,
    T: AsRef<Path>,
{
    type Item = Result<Rename<S, T>, ApplyError>;

    fn next(&mut self) -> Option<Self::Item> {
        let (applied, rename) = self.iter.next()?;
        Some(match rename.apply() {
            Ok(()) => Ok(rename),
            Err(source) => Err(ApplyError {
                source_path: rename.source.as_ref().to_path_buf(),
                target_path: rename.target.as_ref().to_path_buf(),
                source,
                applied,
            }),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<S, T> ExactSizeIterator for ApplyIter<S, T>
where
    S: AsRef<Path>,
    T: AsRef<Path>,
{
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
        assert_eq!(err.source_path, dir.join("c"));
        assert_eq!(err.target_path, dir.join("d"));
    }

    #[test]
    fn check_fs_reports_pre_existing_target() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        File::create(dir.join("a")).unwrap();
        File::create(dir.join("b")).unwrap(); // collides
        File::create(dir.join("c")).unwrap();

        let mut renamer = Renamer::new();
        renamer.add(dir.join("a"), dir.join("b"));
        renamer.add(dir.join("c"), dir.join("d"));

        let mut plan = renamer.plan().unwrap();
        let conflicts = plan.check_fs().unwrap();

        assert_eq!(conflicts.len(), 1);
        match &conflicts[0] {
            crate::FsConflict::TargetExists { target_path } => {
                assert_eq!(target_path, &dir.join("b"));
            }
        }
        assert_eq!(plan.len(), 1);
    }

    #[test]
    fn check_fs_does_not_report_targets_freed_by_the_batch() {
        // a -> b -> c: b's target c is created later, but b itself will be
        // moved out of the way (its source is also a target). check_fs must
        // not flag this as a conflict.
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        File::create(dir.join("a")).unwrap();
        File::create(dir.join("b")).unwrap();

        let mut renamer = Renamer::new();
        renamer.add(dir.join("a"), dir.join("b"));
        renamer.add(dir.join("b"), dir.join("c"));

        let mut plan = renamer.plan().unwrap();
        let conflicts = plan.check_fs().unwrap();

        assert!(conflicts.is_empty());
        assert_eq!(plan.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn check_fs_does_not_report_same_file_via_symlink() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        File::create(dir.join("a")).unwrap();
        // The "target" is a symlink that resolves to the source — same file,
        // no conflict.
        symlink(dir.join("a"), dir.join("b")).unwrap();

        let mut renamer = Renamer::new();
        renamer.add(dir.join("a"), dir.join("b"));

        let mut plan = renamer.plan().unwrap();
        let conflicts = plan.check_fs().unwrap();

        assert!(conflicts.is_empty());
        assert_eq!(plan.len(), 1);
    }

    #[test]
    fn apply_iter_continues_past_failures() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        // Middle rename collides with a pre-existing target; the others go
        // through. Best-effort iteration should yield Ok / Err / Ok in order.
        File::create(dir.join("a")).unwrap();
        File::create(dir.join("c")).unwrap();
        File::create(dir.join("d")).unwrap();
        File::create(dir.join("e")).unwrap();

        let mut renamer = Renamer::new();
        renamer.add(dir.join("a"), dir.join("b"));
        renamer.add(dir.join("c"), dir.join("d"));
        renamer.add(dir.join("e"), dir.join("f"));

        let outcomes: Vec<_> = renamer.plan().unwrap().apply_iter().collect();
        assert_eq!(outcomes.len(), 3);
        assert!(outcomes[0].is_ok());
        let err = outcomes[1].as_ref().unwrap_err();
        assert_eq!(err.applied, 1);
        assert_eq!(err.source_path, dir.join("c"));
        assert_eq!(err.target_path, dir.join("d"));
        assert!(outcomes[2].is_ok());

        assert!(dir.join("b").exists());
        assert!(dir.join("f").exists());
    }
}
