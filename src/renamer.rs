use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
};

use crate::{error::PlanError, operation::Rename, plan::Plan};

/// Prepares a batch file renaming operation.
#[derive(Debug)]
pub struct Renamer<S, T> {
    renames: Vec<Rename<S, T>>,
}

impl<S, T> Renamer<S, T> {
    /// Creates a new [`Renamer`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::path::Path;
    /// # use nominal::Renamer;
    /// let renamer: Renamer<&Path, &Path> = Renamer::new();
    /// ```
    pub fn new() -> Self {
        Self {
            renames: Default::default(),
        }
    }

    /// Creates a new [`Renamer`] with the specified capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::path::Path;
    /// # use nominal::Renamer;
    /// let renamer: Renamer<&Path, &Path> = Renamer::with_capacity(10);
    /// ```
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            renames: Vec::with_capacity(capacity),
        }
    }

    /// Adds a rename operation to the renamer.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::path::Path;
    /// # use nominal::Renamer;
    /// let mut renamer = Renamer::new();
    /// renamer.add("old.txt", "new.txt");
    /// ```
    pub fn add(&mut self, source: S, target: T) {
        self.renames.push(Rename::new(source, target));
    }
}

impl<S, T> Renamer<S, T>
where
    S: AsRef<Path>,
    T: AsRef<Path>,
{
    /// Consumes the renamer and returns a [`Plan`].
    pub fn plan(self) -> Result<Plan<S, T>, PlanError> {
        let mut renames = self.renames;
        renames.retain(|r| r.source.as_ref() != r.target.as_ref());

        let mut seen_sources = HashSet::with_capacity(renames.len());
        for rename in &renames {
            if !seen_sources.insert(rename.source.as_ref()) {
                return Err(PlanError::DuplicateSource {
                    source_path: rename.source.as_ref().to_path_buf(),
                });
            }
        }

        let mut seen_targets = HashSet::with_capacity(renames.len());
        for rename in &renames {
            if !seen_targets.insert(rename.target.as_ref()) {
                return Err(PlanError::DuplicateTarget {
                    target: rename.target.as_ref().to_path_buf(),
                });
            }
        }

        // Sort by target path first, so that independent operations come out in
        // a deterministic order and ties in the topological sort below break by
        // target.
        #[cfg(feature = "unicode")]
        {
            use std::cmp::Ordering;

            use icu_collator::{Collator, CollatorOptions};

            let mut collator_opts = CollatorOptions::new();
            collator_opts.numeric = Some(icu_collator::Numeric::On);
            let collator = Collator::try_new(Default::default(), collator_opts)?;

            #[cfg(unix)]
            fn compare_paths(collator: &Collator, p1: &Path, p2: &Path) -> Ordering {
                use std::os::unix::ffi::OsStrExt;

                collator.compare_utf8(p1.as_os_str().as_bytes(), p2.as_os_str().as_bytes())
            }

            #[cfg(windows)]
            fn compare_paths(collator: &Collator, p1: &Path, p2: &Path) -> Ordering {
                use std::os::windows::ffi::OsStrExt;

                let p1: Vec<u16> = p1.as_os_str().encode_wide().collect();
                let p2: Vec<u16> = p2.as_os_str().encode_wide().collect();
                collator.compare_utf16(&p1, &p2)
            }

            renames
                .sort_by(|r1, r2| compare_paths(&collator, r1.target.as_ref(), r2.target.as_ref()));
        }
        #[cfg(not(feature = "unicode"))]
        {
            renames.sort_by(|r1, r2| r1.target.as_ref().cmp(r2.target.as_ref()));
        }

        topological_sort(&mut renames)?;
        Ok(Plan { renames })
    }
}

/// Reorders renames so that each operation runs after any other operation
/// whose target is its source (which must vacate that path first). Returns
/// [`PlanError::Cycle`] if no such ordering exists.
fn topological_sort<S, T>(renames: &mut [Rename<S, T>]) -> Result<(), PlanError>
where
    S: AsRef<Path>,
    T: AsRef<Path>,
{
    let n = renames.len();
    let target_to_idx: HashMap<&Path, usize> = renames
        .iter()
        .enumerate()
        .map(|(i, r)| (r.target.as_ref(), i))
        .collect();

    let mut indegree = vec![0usize; n];
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, rename) in renames.iter().enumerate() {
        if let Some(&j) = target_to_idx.get(rename.source.as_ref()) {
            // Op j wants to write to a path (T_j = S_i) that op i still reads
            // from. Op i must move it out of the way first: edge i -> j.
            successors[i].push(j);
            indegree[j] += 1;
        }
    }

    // Kahn's algorithm. `dest[i]` ends up as the new position of the element
    // currently at index i.
    let mut dest = vec![0usize; n];
    let mut placed = 0;
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    while let Some(i) = queue.pop_front() {
        dest[i] = placed;
        placed += 1;
        for &j in &successors[i] {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                queue.push_back(j);
            }
        }
    }

    if placed != n {
        let targets = renames
            .iter()
            .enumerate()
            .filter(|(i, _)| indegree[*i] > 0)
            .map(|(_, r)| r.target.as_ref().to_path_buf())
            .collect();
        return Err(PlanError::Cycle { targets });
    }

    // Apply the permutation in place by following cycles.
    for i in 0..n {
        while dest[i] != i {
            let j = dest[i];
            renames.swap(i, j);
            dest.swap(i, j);
        }
    }

    Ok(())
}

impl<S, T> Default for Renamer<S, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S, T> FromIterator<(S, T)> for Renamer<S, T> {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = (S, T)>,
    {
        Self {
            renames: iter.into_iter().map(Into::into).collect(),
        }
    }
}

impl<S, T> Extend<(S, T)> for Renamer<S, T> {
    fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = (S, T)>,
    {
        self.renames.extend(iter.into_iter().map(Into::into));
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::Renamer;
    use crate::error::PlanError;

    #[test]
    fn duplicate_source_is_rejected() {
        let mut renamer = Renamer::new();
        renamer.add("a", "b");
        renamer.add("a", "c");

        match renamer.plan() {
            Err(PlanError::DuplicateSource { source_path }) => {
                assert_eq!(source_path, PathBuf::from("a"));
            }
            other => panic!("expected DuplicateSource, got {:?}", other),
        }
    }

    #[test]
    fn duplicate_target_is_rejected() {
        let mut renamer = Renamer::new();
        renamer.add("a", "z");
        renamer.add("b", "z");

        match renamer.plan() {
            Err(PlanError::DuplicateTarget { target }) => {
                assert_eq!(target, PathBuf::from("z"));
            }
            other => panic!("expected DuplicateTarget, got {:?}", other),
        }
    }

    #[test]
    fn duplicate_noop_is_ignored() {
        // Both entries are no-ops and dropped before the duplicate check.
        let mut renamer = Renamer::new();
        renamer.add("a", "a");
        renamer.add("a", "a");

        let plan = renamer.plan().expect("no-ops should not collide");
        assert!(plan.is_empty());
    }

    #[test]
    fn chain_is_ordered_to_vacate_targets_first() {
        // a -> b -> c: b must run before a, otherwise applying a -> b would
        // collide with b's existing file.
        let mut renamer = Renamer::new();
        renamer.add("a", "b");
        renamer.add("b", "c");

        let plan = renamer.plan().expect("acyclic chain should plan");
        let order: Vec<_> = plan
            .renames
            .iter()
            .map(|r| (r.source, r.target))
            .collect();
        assert_eq!(order, vec![("b", "c"), ("a", "b")]);
    }

    #[test]
    fn cycle_is_rejected() {
        let mut renamer = Renamer::new();
        renamer.add("a", "b");
        renamer.add("b", "a");

        match renamer.plan() {
            Err(PlanError::Cycle { mut targets }) => {
                targets.sort();
                assert_eq!(targets, vec![PathBuf::from("a"), PathBuf::from("b")]);
            }
            other => panic!("expected Cycle, got {:?}", other),
        }
    }
}
