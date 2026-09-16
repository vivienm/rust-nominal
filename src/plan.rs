use std::{collections::HashSet, fs, io, path::Path, vec};

use crate::{
    error::{ApplyError, FsError},
    fsutil::{EntryKey, IdentifiedPath, target_conflicts},
    operation::{Rename, apply_resolved},
    preparation::{Rejection, RejectionTracker},
};

/// A renaming plan.
#[derive(Debug)]
#[must_use]
pub struct Plan<S, T> {
    pub(crate) renames: Vec<PreparedRename<S, T>>,
}

/// A borrowed view of a planned endpoint and its original value.
///
/// [`AsRef<Path>`] returns the captured [`Self::resolved`] path, which is used
/// for execution and conflict checks even if the original value changes through
/// interior mutability. Copying this view neither allocates nor clones its data.
#[derive(Debug)]
pub struct PlannedPath<'a, P: ?Sized> {
    /// The original value supplied to [`crate::Renamer`], including any metadata.
    pub original: &'a P,
    /// The execution path captured during preparation.
    pub resolved: &'a Path,
}

impl<P: ?Sized> Copy for PlannedPath<'_, P> {}

impl<P: ?Sized> Clone for PlannedPath<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: ?Sized> AsRef<Path> for PlannedPath<'_, P> {
    fn as_ref(&self) -> &Path {
        self.resolved
    }
}

/// An operation whose endpoints have both passed path resolution.
/// Keep execution paths and dependency identities attached when reordering or
/// rejecting operations; original values remain available for sorting, inspection
/// and results.
pub(crate) type PreparedRename<S, T> = Rename<PreparedPath<S>, PreparedPath<T>>;

/// The execution spelling and preparation-time identity of one endpoint.
#[derive(Debug)]
pub(crate) struct PreparedPath<P> {
    pub(crate) original: P,
    identified: IdentifiedPath,
}

impl<P> PreparedPath<P> {
    pub(crate) fn new(original: P, identified: IdentifiedPath) -> Self {
        Self {
            original,
            identified,
        }
    }

    pub(crate) fn key(&self) -> &EntryKey {
        self.identified.key()
    }

    fn resolved(&self) -> &Path {
        self.identified.as_path()
    }

    fn view(&self) -> PlannedPath<'_, P> {
        PlannedPath {
            original: &self.original,
            resolved: self.resolved(),
        }
    }
}

impl<S, T> PreparedRename<S, T> {
    fn view(&self) -> Rename<PlannedPath<'_, S>, PlannedPath<'_, T>> {
        Rename::new(self.source.view(), self.target.view())
    }

    pub(crate) fn into_original(self) -> Rename<S, T> {
        Rename::new(self.source.original, self.target.original)
    }

    fn resolved(&self) -> Rename<&Path, &Path> {
        Rename::new(self.source.resolved(), self.target.resolved())
    }
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
    /// # let dir = tempfile::tempdir()?;
    /// # std::fs::write(dir.path().join("old.txt"), "contents")?;
    /// let mut renamer = Renamer::new();
    /// renamer.add(dir.path().join("old.txt"), dir.path().join("new.txt"));
    ///
    /// let plan = renamer.prepare().into_plan()?;
    /// assert_eq!(plan.len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn len(&self) -> usize {
        self.renames.len()
    }

    /// Yields borrowed views of the retained operations in execution order.
    ///
    /// Each view is a [`Rename`] of two [`PlannedPath`] values, exposing both the
    /// original values supplied to [`crate::Renamer`] and the captured execution
    /// paths. Creating views neither allocates nor clones those values.
    /// Iteration neither consumes the plan nor inspects or modifies the filesystem.
    /// Execution and conflict checks use the paths captured during preparation,
    /// even if interior mutability changes the original values exposed here.
    ///
    /// # Examples
    ///
    /// ```
    /// # use nominal::Renamer;
    /// # let dir = tempfile::tempdir()?;
    /// # std::fs::write(dir.path().join("old"), "contents")?;
    /// let plan = Renamer::from_iter([(dir.path().join("old"), dir.path().join("new"))])
    ///     .prepare()
    ///     .into_plan()?;
    /// for rename in plan.iter() {
    ///     println!("{rename}"); // Displays the captured paths.
    /// }
    /// assert_eq!(plan.len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = Rename<PlannedPath<'_, S>, PlannedPath<'_, T>>>
    + DoubleEndedIterator {
        self.renames.iter().map(PreparedRename::view)
    }

    /// Writes the plan's captured execution paths to the specified writer.
    pub fn write_to<W>(&self, writer: &mut W) -> io::Result<()>
    where
        W: io::Write,
    {
        for rename in self.iter() {
            rename.write_to(writer)?;
        }
        Ok(())
    }

    /// Writes the plan's captured execution paths to the specified writer, with ANSI colors.
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
        for rename in self.iter() {
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

    /// Rechecks sources and destinations and removes operations with filesystem errors.
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
    /// Sources must exist, including dangling symlinks. A missing source must
    /// not consume a destination created by another operation in this batch.
    /// No-ops have already been removed and are not inspected.
    /// This preflight check does not reserve destinations or lock sources.
    /// Execution also refuses replacement atomically. Later source removal,
    /// permissions and cross-filesystem moves can still cause execution errors.
    pub fn reject_conflicts(&mut self) -> Vec<Rejection<S, T>> {
        let mut vacated: HashSet<&EntryKey> = HashSet::with_capacity(self.renames.len());
        let mut rejected = RejectionTracker::new(self.renames.len());
        for (index, rename) in self.renames.iter().enumerate() {
            let resolved = rename.resolved();
            let conflict = fs::symlink_metadata(resolved.source).and_then(|_| {
                if vacated.contains(rename.target.key()) {
                    Ok(false)
                } else {
                    target_conflicts(resolved.source, resolved.target)
                }
            });
            match conflict {
                Ok(false) => {
                    vacated.insert(rename.source.key());
                }
                Ok(true) => rejected.mark(
                    [index],
                    FsError::TargetExists {
                        target_path: resolved.target.to_path_buf(),
                    },
                ),
                Err(source) => rejected.mark(
                    [index],
                    FsError::Inspect {
                        source_path: resolved.source.to_path_buf(),
                        target_path: resolved.target.to_path_buf(),
                        source,
                    },
                ),
            }
        }
        let (retained, rejections) = rejected.partition(
            std::mem::take(&mut self.renames),
            PreparedRename::into_original,
        );
        self.renames = retained;
        rejections
    }

    /// Executes the plan, stopping at the first failure.
    ///
    /// Each system rename atomically refuses to replace an existing target,
    /// including one created after preparation. See [`Rename::apply`] for
    /// platform support. Paths and source identities are not locked against
    /// concurrent changes, and the batch as a whole is not atomic.
    /// Execution uses the captured paths without resolving their parents again,
    /// while checking destination occupancy against the current filesystem.
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
    iter: vec::IntoIter<PreparedRename<S, T>>,
}

impl<S, T> Iterator for ApplyIter<S, T> {
    type Item = Result<Rename<S, T>, ApplyError>;

    fn next(&mut self) -> Option<Self::Item> {
        let rename = self.iter.next()?;
        let result = apply_resolved(
            rename.source.identified.resolved(),
            rename.target.identified.resolved(),
        );
        Some(match result {
            Ok(()) => Ok(rename.into_original()),
            Err(source) => Err(ApplyError {
                source_path: rename.source.identified.into_resolved().into(),
                target_path: rename.target.identified.into_resolved().into(),
                source,
            }),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<S, T> ExactSizeIterator for ApplyIter<S, T> {}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use crate::Renamer;

    #[test]
    fn execution_paths_resolve_aliases_and_preserve_original_values() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let source = root.join("source");
        let target = root.join("./target");
        std::fs::write(&source, "contents").unwrap();
        // Exercise an owned source and a borrowed target in the same operation.
        let mut plan = Renamer::from_iter([(source, &target)])
            .prepare()
            .into_plan()
            .unwrap();
        let rename = &plan.renames[0];
        let resolved = rename.resolved();
        assert_eq!(
            resolved.source.as_os_str(),
            rename.source.original.as_os_str()
        );
        assert_eq!(resolved.target.as_os_str(), root.join("target").as_os_str());
        assert_ne!(resolved.target.as_os_str(), target.as_os_str());
        assert!(std::ptr::eq(
            *plan.iter().next().unwrap().target.original,
            &target
        ));

        assert!(plan.reject_conflicts().is_empty());
        plan.apply().unwrap();
        assert!(!root.join("source").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("target")).unwrap(),
            "contents"
        );
    }

    #[test]
    fn apply_reports_failure_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path().canonicalize().unwrap();

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
        let dir = temp_dir.path().canonicalize().unwrap();

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
        let dir = temp_dir.path().canonicalize().unwrap();

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
