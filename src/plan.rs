use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    vec,
};

use crate::{
    error::{ApplyError, FsError},
    fsutil::{EntryKey, target_conflicts},
    operation::Rename,
    preparation::{Rejection, RejectionTracker},
};

/// A renaming plan.
#[derive(Debug)]
#[must_use]
pub struct Plan<S, T> {
    pub(crate) renames: Vec<Rename<S, T>>,
    pub(crate) paths: HashMap<OsString, PathBuf>,
    pub(crate) keys: HashMap<OsString, EntryKey>,
}

impl<S, T> Plan<S, T> {
    /// Returns `true` if the plan is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// # use nominal::{Plan, Renamer};
    /// let plan: Plan<&str, &str> = Renamer::new().prepare().into_plan()?;
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
    /// let plan = renamer.prepare().into_plan()?;
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
    /// let plan: Plan<&str, &str> = Renamer::new().prepare().into_plan()?;
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

    /// Rechecks destinations and removes operations with filesystem conflicts.
    ///
    /// Preparation already performs this check. Call it again if the filesystem
    /// may have changed before execution. Each rejection includes the original
    /// operation and either an occupied-target diagnostic or an inspection I/O
    /// error. An inspection failure rejects that operation without discarding
    /// independent operations. Conflicts propagate through chains: a rejected
    /// `b -> c` cannot unblock `a -> b` while `b` is still occupied.
    ///
    /// Existing symlinks (including dangling ones) and separate hard links are
    /// conflicts. Case-only changes of singly linked files are allowed on
    /// case-insensitive filesystems. On Unix, multiply linked files are rejected
    /// conservatively for existing targets, even for case-only changes. On other
    /// platforms, existing symlink targets are always rejected.
    ///
    /// This preflight check does not reserve destinations. Execution also
    /// refuses replacement atomically. Source existence, permissions and
    /// cross-filesystem moves can still fail at execution time.
    pub fn reject_conflicts(&mut self) -> Vec<Rejection<S, T>> {
        let mut vacated: HashSet<&EntryKey> = HashSet::with_capacity(self.renames.len());
        let mut rejected = RejectionTracker::new(self.renames.len());
        for (index, rename) in self.renames.iter().enumerate() {
            let source = self.paths[rename.source.as_ref().as_os_str()].as_path();
            let target = self.paths[rename.target.as_ref().as_os_str()].as_path();
            let conflict = if vacated.contains(&self.keys[rename.target.as_ref().as_os_str()]) {
                Ok(false)
            } else {
                target_conflicts(source, target)
            };
            match conflict {
                Ok(false) => {
                    vacated.insert(&self.keys[rename.source.as_ref().as_os_str()]);
                }
                Ok(true) => rejected.mark(
                    [index],
                    FsError::TargetExists {
                        target_path: rename.target.as_ref().to_path_buf(),
                    },
                ),
                Err(source) => rejected.mark(
                    [index],
                    FsError::Inspect {
                        source_path: rename.source.as_ref().to_path_buf(),
                        target_path: rename.target.as_ref().to_path_buf(),
                        source,
                    },
                ),
            }
        }
        let (retained, rejections) = rejected.partition(std::mem::take(&mut self.renames));
        self.renames = retained;
        rejections
    }

    /// Executes the plan, stopping at the first failure.
    ///
    /// Each system rename atomically refuses to replace an existing target,
    /// including one created after preparation. See [`Rename::apply`] for
    /// platform support. Paths and source identities are not locked against
    /// concurrent changes, and the batch as a whole is not atomic.
    ///
    /// Case-only changes may use a temporary name; see [`Rename::apply`]
    /// for visibility and recovery behavior during those operations.
    ///
    /// If a rename fails partway through, the operations applied so far are
    /// not rolled back.
    ///
    /// To continue past failures, or to know how many renames succeeded
    /// before a failure, use [`apply_iter`](Self::apply_iter) instead.
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
    /// let plan = renamer.prepare().into_plan()?;
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
    /// (best-effort) or stop early. The concurrent-change and partial-application
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
    ///
    /// let mut renamer = Renamer::new();
    /// renamer.add(dir.join("a"), dir.join("b"));
    /// renamer.add(dir.join("c"), dir.join("d"));
    ///
    /// let mut renamed = 0;
    /// let mut errors = Vec::new();
    /// let plan = renamer.prepare().into_plan()?;
    /// File::create(dir.join("d"))?; // target appeared after preparation
    /// for result in plan.apply_iter() {
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
            iter: self.renames.into_iter(),
            paths: self.paths,
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
    iter: vec::IntoIter<Rename<S, T>>,
    paths: HashMap<OsString, PathBuf>,
}

impl<S, T> Iterator for ApplyIter<S, T>
where
    S: AsRef<Path>,
    T: AsRef<Path>,
{
    type Item = Result<Rename<S, T>, ApplyError>;

    fn next(&mut self) -> Option<Self::Item> {
        let rename = self.iter.next()?;
        let resolved = Rename::new(
            &self.paths[rename.source.as_ref().as_os_str()],
            &self.paths[rename.target.as_ref().as_os_str()],
        );
        Some(match resolved.apply() {
            Ok(()) => Ok(rename),
            Err(source) => Err(ApplyError {
                source_path: rename.source.as_ref().to_path_buf(),
                target_path: rename.target.as_ref().to_path_buf(),
                source,
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
    fn apply_reports_failure_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        // First rename succeeds, second collides with a pre-existing target.
        File::create(dir.join("a")).unwrap();
        File::create(dir.join("c")).unwrap();

        let mut renamer = Renamer::new();
        renamer.add(dir.join("a"), dir.join("b"));
        renamer.add(dir.join("c"), dir.join("d"));

        let plan = renamer.prepare().into_plan().unwrap();
        File::create(dir.join("d")).unwrap();
        let err = plan.apply().expect_err("second rename should fail");
        assert_eq!(err.source_path, dir.join("c"));
        assert_eq!(err.target_path, dir.join("d"));
    }

    #[test]
    fn preparation_reports_pre_existing_target() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        File::create(dir.join("a")).unwrap();
        File::create(dir.join("b")).unwrap(); // collides
        File::create(dir.join("c")).unwrap();

        let mut renamer = Renamer::new();
        renamer.add(dir.join("a"), dir.join("b"));
        renamer.add(dir.join("c"), dir.join("d"));

        let (plan, conflicts) = renamer.prepare().into_parts();

        assert_eq!(conflicts.len(), 1);
        assert!(matches!(&conflicts[0].reason,
            crate::RejectionReason::Filesystem(crate::FsError::TargetExists { target_path }) if target_path == &dir.join("b")));
        assert_eq!(plan.len(), 1);
    }

    #[test]
    fn preparation_does_not_report_targets_freed_by_the_batch() {
        // a -> b -> c: b's target c is created later, but b itself will be
        // moved out of the way (its source is also a target). preparation must
        // not flag this as a conflict.
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        File::create(dir.join("a")).unwrap();
        File::create(dir.join("b")).unwrap();

        let mut renamer = Renamer::new();
        renamer.add(dir.join("a"), dir.join("b"));
        renamer.add(dir.join("b"), dir.join("c"));

        let (plan, conflicts) = renamer.prepare().into_parts();

        assert!(conflicts.is_empty());
        assert_eq!(plan.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn preparation_reports_target_symlink_to_source() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        File::create(dir.join("a")).unwrap();
        // A symlink to the source is still a distinct directory entry.
        symlink(dir.join("a"), dir.join("b")).unwrap();

        let mut renamer = Renamer::new();
        renamer.add(dir.join("a"), dir.join("b"));

        let (plan, conflicts) = renamer.prepare().into_parts();

        assert_eq!(conflicts.len(), 1);
        assert!(plan.is_empty());
    }

    #[test]
    fn apply_iter_continues_past_failures() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        // Middle rename collides with a pre-existing target; the others go
        // through. Best-effort iteration should yield Ok / Err / Ok in order.
        File::create(dir.join("a")).unwrap();
        File::create(dir.join("c")).unwrap();
        File::create(dir.join("e")).unwrap();

        let mut renamer = Renamer::new();
        renamer.add(dir.join("a"), dir.join("b"));
        renamer.add(dir.join("c"), dir.join("d"));
        renamer.add(dir.join("e"), dir.join("f"));

        let plan = renamer.prepare().into_plan().unwrap();
        File::create(dir.join("d")).unwrap();
        let outcomes: Vec<_> = plan.apply_iter().collect();
        assert_eq!(outcomes.len(), 3);
        assert!(outcomes[0].is_ok());
        let err = outcomes[1].as_ref().unwrap_err();
        assert_eq!(err.source_path, dir.join("c"));
        assert_eq!(err.target_path, dir.join("d"));
        assert!(outcomes[2].is_ok());

        assert!(dir.join("b").exists());
        assert!(dir.join("f").exists());
    }
}
