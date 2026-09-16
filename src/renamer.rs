use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    path::Path,
};

use crate::{
    error::PlanError,
    fsutil::{EntryKey, entry_key},
    operation::Rename,
    path_cache::{InspectedRename, ResolutionCache},
    plan::{Plan, PreparedRename},
    preparation::{Preparation, Rejection, RejectionTracker},
};

/// Prepares a batch file renaming operation.
#[derive(Debug)]
#[must_use]
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
            renames: Vec::new(),
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
    /// Prepares the batch and returns a report without changing the filesystem.
    ///
    /// [`Preparation::into_plan`] returns a plan only if there are no rejections.
    /// [`Preparation::into_parts`] also exposes a partial plan for callers that
    /// want to continue with independent valid operations. No-op operations
    /// are omitted; every rejected operation is reported exactly once.
    /// Duplicate contenders are all rejected, including intersecting groups.
    /// Resolvable endpoints still participate in duplicate and overlap checks
    /// when an operation's other endpoint cannot be resolved.
    /// If both endpoints fail path inspection, the source error takes priority.
    /// Occupied targets and filesystem inspection failures are collected too.
    /// Non-noop sources must exist; dangling symlinks count as existing entries.
    /// Missing sources are rejected before any operation is executed.
    /// Rejected operations cannot unblock dependent renames.
    ///
    /// Paths are anchored to the current directory at planning time. Existing
    /// parent directories are resolved to handle `..` and symlink aliases;
    /// missing parent directories are allowed. The final component is kept
    /// unchanged so symlinks themselves can be renamed. Original paths are
    /// preserved for inspection and results; plan output uses the captured paths.
    ///
    /// Paths are not confined to a library or working directory: absolute
    /// destinations and `..` in existing parent directories are supported.
    /// Callers deriving destinations from metadata or other external input
    /// must validate those destinations against their own directory policy.
    /// Resolving aliases is not a confinement check.
    ///
    /// Existing case aliases are identified from filesystem metadata, using
    /// the same conservative hard-link and symlink rules as [`Plan::reject_conflicts`].
    /// Missing names are compared by spelling; case aliases between missing
    /// names can therefore collide only at execution time.
    ///
    /// A batch cannot contain a source or target that is a strict ancestor
    /// of another source or target. This includes nested destinations such
    /// as `out` and `out/child`, even when the source of `out` is a directory.
    /// Rename directories and their descendants in separate batches.
    ///
    /// A `..` component after a missing directory is rejected because its
    /// filesystem meaning cannot be resolved. Sorting failures reject all
    /// otherwise retained operations rather than expose an unordered plan.
    pub fn prepare(self) -> Preparation<S, T> {
        let mut cache = ResolutionCache::with_capacity(2 * self.renames.len());
        let mut renames = Vec::with_capacity(self.renames.len());
        for rename in self.renames {
            if rename.source.as_ref().as_os_str() == rename.target.as_ref().as_os_str() {
                continue;
            }
            // Resolve both endpoints before comparing execution spellings.
            // Failures remain in the cache for the identification phase.
            cache.resolve(rename.source.as_ref());
            cache.resolve(rename.target.as_ref());
            if let (Some(source), Some(target)) = (
                cache.resolved(rename.source.as_ref()),
                cache.resolved(rename.target.as_ref()),
            ) && source.as_os_str() == target.as_os_str()
            {
                continue;
            }
            renames.push(rename);
        }

        let mut cache = cache.into_identification();
        let renames: Vec<_> = renames
            .into_iter()
            .map(|rename| Rename::new(cache.inspect(rename.source), cache.inspect(rename.target)))
            .collect();
        // Every endpoint now carries its own result. Releasing the cache also
        // lets the final owner of an identified path transfer its allocations.
        drop(cache);

        let rejected = validate_paths(&renames);
        let (retained, mut rejections) =
            rejected.partition(renames, InspectedRename::into_original);

        let mut renames = Vec::with_capacity(retained.len());
        for rename in retained {
            match rename.into_prepared() {
                Ok(rename) => renames.push(rename),
                Err(rejection) => rejections.push(rejection),
            }
        }

        if !renames.is_empty() {
            if let Err(error) = sort_by_target(&mut renames) {
                rejections.push(Rejection {
                    renames: std::mem::take(&mut renames)
                        .into_iter()
                        .map(PreparedRename::into_original)
                        .collect(),
                    reason: error.into(),
                });
            } else if let Err(cycles) = topological_sort(&mut renames) {
                // All cycles are known after one pass. Remove every cycle in
                // one partition, then order the remaining acyclic operations.
                let mut rejected = RejectionTracker::new(renames.len());
                for cycle in cycles {
                    let paths = cycle
                        .iter()
                        .map(|&i| renames[i].target.original.as_ref().to_path_buf())
                        .collect();
                    rejected.mark(cycle, PlanError::Cycle { paths });
                }
                let (retained, rejected) =
                    rejected.partition(renames, PreparedRename::into_original);
                renames = retained;
                rejections.extend(rejected);
                topological_sort(&mut renames).expect("all cycles were removed");
            }
        }
        let mut plan = Plan { renames };
        rejections.extend(plan.reject_conflicts());
        Preparation { plan, rejections }
    }
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

/// Most endpoints belong to a single operation. Allocate only when another
/// operation shares the key, keeping each group nonempty and in input order.
enum OperationIndices {
    One(usize),
    Many(Vec<usize>),
}

impl OperationIndices {
    fn push(&mut self, index: usize) {
        match self {
            Self::One(first) => *self = Self::Many(vec![*first, index]),
            Self::Many(indices) => indices.push(index),
        }
    }

    fn as_slice(&self) -> &[usize] {
        match self {
            Self::One(index) => std::slice::from_ref(index),
            Self::Many(indices) => indices,
        }
    }
}

type EndpointIndex<'a> = HashMap<&'a EntryKey, OperationIndices>;

fn index_endpoint<'a>(index: &mut EndpointIndex<'a>, key: &'a EntryKey, operation: usize) {
    index
        .entry(key)
        .and_modify(|indices| indices.push(operation))
        .or_insert(OperationIndices::One(operation));
}

/// Collect path errors, duplicates and ancestor overlaps across the full batch.
/// Invalid operations still participate through their successfully inspected paths.
fn validate_paths<S: AsRef<Path>, T: AsRef<Path>>(
    renames: &[InspectedRename<S, T>],
) -> RejectionTracker {
    let mut rejected = RejectionTracker::new(renames.len());
    for (index, rename) in renames.iter().enumerate() {
        if let Some(error) = rename.plan_error() {
            rejected.mark([index], error);
        }
    }
    let mut sources: EndpointIndex<'_> = HashMap::new();
    let mut targets: EndpointIndex<'_> = HashMap::new();
    let mut endpoints: EndpointIndex<'_> = HashMap::new();
    for (index, rename) in renames.iter().enumerate() {
        let source = rename.source.key();
        let target = rename.target.key();
        if let Some(source) = source {
            index_endpoint(&mut sources, source, index);
            index_endpoint(&mut endpoints, source, index);
        }
        if let Some(target) = target {
            index_endpoint(&mut targets, target, index);
            if Some(target) != source {
                index_endpoint(&mut endpoints, target, index);
            }
        }
    }
    // Keep the full input and a shared tracker: rejected operations must still
    // participate in later checks, and the first diagnostic takes precedence.
    reject_duplicates(renames, &sources, &targets, &mut rejected);
    reject_overlaps(renames, &endpoints, &mut rejected);
    rejected
}

fn reject_duplicates<S: AsRef<Path>, T: AsRef<Path>>(
    renames: &[InspectedRename<S, T>],
    sources: &EndpointIndex<'_>,
    targets: &EndpointIndex<'_>,
    rejected: &mut RejectionTracker,
) {
    // Visit in input order, not HashMap iteration order, for stable reports.
    for (index, rename) in renames.iter().enumerate() {
        if let Some(key) = rename.source.key()
            && let source_group = sources[key].as_slice()
            && source_group.len() > 1
            && source_group[0] == index
        {
            rejected.mark(
                source_group.iter().copied(),
                PlanError::DuplicateSource {
                    path: rename.source.original().as_ref().to_path_buf(),
                },
            );
        }
        if let Some(key) = rename.target.key()
            && let target_group = targets[key].as_slice()
            && target_group.len() > 1
            && target_group[0] == index
        {
            rejected.mark(
                target_group.iter().copied(),
                PlanError::DuplicateTarget {
                    path: rename.target.original().as_ref().to_path_buf(),
                },
            );
        }
    }
}

fn reject_overlaps<S: AsRef<Path>, T: AsRef<Path>>(
    renames: &[InspectedRename<S, T>],
    endpoints: &EndpointIndex<'_>,
    rejected: &mut RejectionTracker,
) {
    // Shared directories need only one successful inspection during overlap
    // validation. Borrow paths from the batch and discard this cache afterwards:
    // later preparation, conflict checks and execution must inspect afresh.
    let mut ancestor_keys = HashMap::new();
    for (index, rename) in renames.iter().enumerate() {
        for (path, resolved) in [
            (rename.source.original().as_ref(), rename.source.resolved()),
            (rename.target.original().as_ref(), rename.target.resolved()),
        ] {
            let Some(resolved) = resolved else {
                continue;
            };
            for ancestor in resolved.ancestors().skip(1) {
                let key: &EntryKey = match ancestor_keys.entry(ancestor) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(entry) => match entry_key(ancestor) {
                        Ok(key) => entry.insert(key),
                        Err(source) => {
                            rejected.mark(
                                [index],
                                PlanError::ResolvePath {
                                    path: path.to_path_buf(),
                                    source,
                                },
                            );
                            break;
                        }
                    },
                };
                if let Some(owners) = endpoints.get(key) {
                    let owners = owners.as_slice();
                    let owner = &renames[owners[0]];
                    let original = if owner.source.key() == Some(key) {
                        owner.source.original().as_ref()
                    } else {
                        owner.target.original().as_ref()
                    };
                    rejected.mark(
                        owners.iter().copied().chain([index]),
                        PlanError::OverlappingPaths {
                            ancestor_path: original.to_path_buf(),
                            descendant_path: path.to_path_buf(),
                        },
                    );
                }
            }
        }
    }
}

/// Natural ordering with a lexical tie-break for distinct equivalent names.
fn sort_by_target<S: AsRef<Path>, T: AsRef<Path>>(
    renames: &mut [PreparedRename<S, T>],
) -> Result<(), PlanError> {
    #[cfg(feature = "unicode")]
    {
        use icu_collator::{
            Collator, CollatorPreferences, options::CollatorOptions,
            preferences::CollationNumericOrdering,
        };

        let mut prefs = CollatorPreferences::default();
        prefs.numeric_ordering = Some(CollationNumericOrdering::True);
        let collator = Collator::try_new(prefs, CollatorOptions::default())?;

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;

            renames.sort_by(|r1, r2| {
                let (a, b) = (r1.target.original.as_ref(), r2.target.original.as_ref());
                collator
                    .compare_utf8(a.as_os_str().as_bytes(), b.as_os_str().as_bytes())
                    .then_with(|| a.as_os_str().cmp(b.as_os_str()))
            });
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;

            sort_by_cached_utf16(renames, &collator, |path| {
                path.as_os_str().encode_wide().collect()
            });
        }
    }
    #[cfg(not(feature = "unicode"))]
    {
        renames.sort_by(|r1, r2| r1.target.original.as_ref().cmp(r2.target.original.as_ref()));
    }

    Ok(())
}

/// Encode each target once rather than allocating two UTF-16 buffers per
/// comparison. Sort indices first, keeping the original operations in place
/// until the buffers and their borrowed input paths are no longer needed.
#[cfg(all(feature = "unicode", any(windows, test)))]
fn sort_by_cached_utf16<S, T: AsRef<Path>>(
    renames: &mut [PreparedRename<S, T>],
    collator: &icu_collator::CollatorBorrowed<'_>,
    encode: impl Fn(&Path) -> Vec<u16>,
) {
    let mut order: Vec<_> = renames
        .iter()
        .enumerate()
        .map(|(index, rename)| {
            let path = rename.target.original.as_ref();
            (index, path, encode(path))
        })
        .collect();
    order.sort_by(|(_, a, a_wide), (_, b, b_wide)| {
        collator
            .compare_utf16(a_wide, b_wide)
            .then_with(|| a.as_os_str().cmp(b.as_os_str()))
    });
    let mut new_positions = vec![0; renames.len()];
    for (position, (index, _, _)) in order.into_iter().enumerate() {
        new_positions[index] = position;
    }
    apply_permutation(renames, &mut new_positions);
}

/// Reorders renames so an operation that vacates a target runs before the
/// operation that writes to that target. On failure, leaves the slice unchanged
/// and returns the disjoint cycles as indices into that slice.
fn topological_sort<S, T>(renames: &mut [PreparedRename<S, T>]) -> Result<(), Vec<Vec<usize>>> {
    let n = renames.len();
    let target_to_idx: HashMap<&EntryKey, usize> = renames
        .iter()
        .enumerate()
        .map(|(i, r)| (r.target.key(), i))
        .collect();

    let mut indegree = vec![0usize; n];
    // Each rename has a single source, so at most one outgoing edge.
    let mut successor: Vec<Option<usize>> = vec![None; n];
    for (i, rename) in renames.iter().enumerate() {
        if let Some(&j) = target_to_idx.get(rename.source.key()) {
            // Spelling-only operations can refer to their own entry.
            if i == j {
                continue;
            }
            // Op j wants to write to a path (T_j = S_i) that op i still reads
            // from. Op i must move it out of the way first: edge i -> j.
            successor[i] = Some(j);
            indegree[j] += 1;
        }
    }

    // Kahn's algorithm. `new_positions[i]` ends up as the new position of the element
    // currently at index i.
    let mut new_positions = vec![0usize; n];
    let mut placed = 0;
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    while let Some(i) = queue.pop_front() {
        new_positions[i] = placed;
        placed += 1;
        if let Some(j) = successor[i] {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                queue.push_back(j);
            }
        }
    }

    if placed != n {
        // Each remaining node has indegree 1 and exactly one successor (every
        // target is unique, every rename has one source), so the leftover
        // graph decomposes into disjoint simple cycles — walk each one by
        // following `successor` until we come back to the start.
        let mut visited = vec![false; n];
        let mut cycles = Vec::new();
        for start in 0..n {
            if visited[start] || indegree[start] == 0 {
                continue;
            }
            let mut cycle = Vec::new();
            let mut i = start;
            loop {
                visited[i] = true;
                cycle.push(i);
                i = successor[i].expect("nodes left after Kahn's algorithm have a successor");
                if i == start {
                    break;
                }
            }
            cycles.push(cycle);
        }
        return Err(cycles);
    }

    apply_permutation(renames, &mut new_positions);
    Ok(())
}

/// Move each original element `i` to `new_positions[i]` in place. The supplied
/// permutation is consumed by swapping its entries alongside the values.
fn apply_permutation<T>(values: &mut [T], new_positions: &mut [usize]) {
    for i in 0..values.len() {
        while new_positions[i] != i {
            let j = new_positions[i];
            values.swap(i, j);
            new_positions.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Renamer;
    use crate::{PlanError, RejectionReason};

    #[cfg(feature = "unicode")]
    #[test]
    fn cached_utf16_sort_encodes_once_and_preserves_rename_pairs() {
        use std::{cell::Cell, fs};

        use icu_collator::{
            Collator, CollatorPreferences, options::CollatorOptions,
            preferences::CollationNumericOrdering,
        };

        let dir = tempfile::tempdir().unwrap();
        let pairs = [
            ("a", "photo📷10.jpg"),
            ("b", "photo📷2.jpg"),
            ("c", "photo📷1.jpg"),
            ("d", "photo📷01.jpg"),
        ];
        for (source, _) in pairs {
            fs::write(dir.path().join(source), source).unwrap();
        }
        let mut plan = Renamer::from_iter(
            pairs.map(|(source, target)| (dir.path().join(source), dir.path().join(target))),
        )
        .prepare()
        .into_plan()
        .unwrap();
        // Exercise a permutation cycle longer than a single swap.
        plan.renames.rotate_left(1);
        let mut prefs = CollatorPreferences::default();
        prefs.numeric_ordering = Some(CollationNumericOrdering::True);
        let collator = Collator::try_new(prefs, CollatorOptions::default()).unwrap();
        let encoded = Cell::new(0);
        // Exercise the Windows sorting algorithm on every test platform,
        // including numeric ties and a character represented by two UTF-16 units.
        super::sort_by_cached_utf16(&mut plan.renames, &collator, |path| {
            encoded.set(encoded.get() + 1);
            path.to_str().unwrap().encode_utf16().collect()
        });
        assert_eq!(encoded.get(), pairs.len());
        let sources: Vec<_> = plan
            .iter()
            .map(|rename| rename.source.resolved.file_name().unwrap())
            .collect();
        assert_eq!(sources, ["d", "c", "b", "a"]);
        plan.apply().unwrap();
        for (source, target) in pairs {
            assert_eq!(fs::read_to_string(dir.path().join(target)).unwrap(), source);
        }
    }

    #[test]
    fn duplicates_reject_every_contender() {
        for (pairs, source) in [
            ([("a", "b"), ("a", "c")], true),
            ([("a", "z"), ("b", "z")], false),
        ] {
            let report = Renamer::from_iter(pairs).prepare();
            assert_eq!(
                report.rejections().iter().flat_map(|r| &r.renames).count(),
                2
            );
            assert!(
                matches!(&report.rejections()[0].reason,
                RejectionReason::Plan(PlanError::DuplicateSource { .. }) if source)
                    || matches!(&report.rejections()[0].reason,
                RejectionReason::Plan(PlanError::DuplicateTarget { .. }) if !source)
            );
            assert!(report.into_plan().is_err());
        }
    }

    #[test]
    fn duplicate_noop_is_ignored() {
        let plan = Renamer::from_iter([("a", "a"), ("a", "a")])
            .prepare()
            .into_plan()
            .unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn chain_is_ordered_to_vacate_targets_first() {
        let dir = tempfile::tempdir().unwrap();
        let p = |name: &str| dir.path().join(name);
        std::fs::write(p("a"), "A").unwrap();
        std::fs::write(p("b"), "B").unwrap();
        let plan = Renamer::from_iter([(p("a"), p("b")), (p("b"), p("c"))])
            .prepare()
            .into_plan()
            .unwrap();
        let order: Vec<_> = plan
            .iter()
            .map(|r| (r.source.original.clone(), r.target.original.clone()))
            .collect();
        assert_eq!(order, [(p("b"), p("c")), (p("a"), p("b"))]);
    }

    #[test]
    fn disjoint_cycles_are_grouped_separately() {
        let report = Renamer::from_iter([("a", "b"), ("b", "a"), ("c", "d"), ("d", "c")]).prepare();
        assert_eq!(
            report.rejections().iter().flat_map(|r| &r.renames).count(),
            4
        );
        assert_eq!(report.rejections().len(), 2);
        for rejection in report.rejections() {
            assert_eq!(rejection.renames.len(), 2);
            assert!(
                matches!(&rejection.reason, RejectionReason::Plan(PlanError::Cycle { paths }) if paths.len() == 2)
            );
            let RejectionReason::Plan(PlanError::Cycle { paths }) = &rejection.reason else {
                unreachable!();
            };
            let targets: Vec<_> = rejection
                .renames
                .iter()
                .map(|rename| std::path::PathBuf::from(rename.target))
                .collect();
            let mut paths = paths.clone();
            paths.sort();
            assert_eq!(paths, targets);
        }
        assert!(report.into_parts().0.is_empty());
    }
}
