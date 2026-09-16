use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

use crate::{FsError, Plan, PlanError, Rename};

/// The outcome of preparing a batch, before any files are changed.
///
/// Use [`into_plan`](Self::into_plan) to reject the entire batch if any operation
/// was rejected, or [`into_parts`](Self::into_parts) to process the retained
/// operations and report the rejections yourself.
#[derive(Debug)]
#[must_use]
pub struct Preparation<S, T> {
    pub(crate) plan: Plan<S, T>,
    pub(crate) rejections: Vec<Rejection<S, T>>,
}

impl<S, T> Preparation<S, T> {
    /// The rejected operations, grouped by diagnostic. Each operation occurs
    /// in at most one group, even when it has several problems.
    pub fn rejections(&self) -> &[Rejection<S, T>] {
        &self.rejections
    }

    /// Returns the plan only if every non-noop operation passed preparation.
    /// The error owns the paths of all rejected operations and their diagnostics,
    /// so it can be propagated even when the input paths were borrowed.
    /// No renames are executed by this method. This is a convenience wrapper
    /// around [`Plan::try_from`]; `TryInto<Plan<S, T>>` is available too.
    pub fn into_plan(self) -> Result<Plan<S, T>, PreparationError>
    where
        S: AsRef<Path>,
        T: AsRef<Path>,
    {
        self.try_into()
    }

    /// Returns the retained plan and all rejections for best-effort callers.
    /// Inspect and report the rejections before executing the partial plan.
    pub fn into_parts(self) -> (Plan<S, T>, Vec<Rejection<S, T>>) {
        (self.plan, self.rejections)
    }
}

/// Strict conversion of a preparation report into an executable plan.
///
/// This also provides `TryInto<Plan<S, T>>` for `Preparation<S, T>`.
impl<S: AsRef<Path>, T: AsRef<Path>> TryFrom<Preparation<S, T>> for Plan<S, T> {
    type Error = PreparationError;

    fn try_from(preparation: Preparation<S, T>) -> Result<Self, Self::Error> {
        if preparation.rejections.is_empty() {
            Ok(preparation.plan)
        } else {
            Err(PreparationError {
                rejections: preparation
                    .rejections
                    .into_iter()
                    .map(|rejection| Rejection {
                        reason: rejection.reason,
                        renames: rejection
                            .renames
                            .into_iter()
                            .map(|rename| {
                                Rename::new(
                                    rename.source.as_ref().to_path_buf(),
                                    rename.target.as_ref().to_path_buf(),
                                )
                            })
                            .collect(),
                    })
                    .collect(),
            })
        }
    }
}

/// Operations rejected for a common reason during preparation.
#[derive(Debug)]
pub struct Rejection<S, T> {
    /// Rejected operations with their original source and target values.
    pub renames: Vec<Rename<S, T>>,
    /// The diagnostic responsible for rejecting these operations.
    pub reason: RejectionReason,
}

/// A diagnostic reported during batch preparation.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
#[non_exhaustive]
pub enum RejectionReason {
    /// Invalid paths, duplicate endpoints, overlaps, cycles or sorting failures.
    Plan(#[from] PlanError),
    /// An occupied destination or a filesystem inspection failure.
    Filesystem(#[from] FsError),
}

/// All rejections from a strict [`Preparation::into_plan`] conversion.
///
/// [`Display`](fmt::Display) produces a single-line summary. Applications can
/// inspect [`rejections`](Self::rejections) to present individual diagnostics.
/// Owned paths let this error outlive the preparation's borrowed inputs.
#[derive(Debug)]
pub struct PreparationError {
    /// Rejected operations grouped by diagnostic.
    pub rejections: Vec<Rejection<PathBuf, PathBuf>>,
}

impl fmt::Display for PreparationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count: usize = self.rejections.iter().map(|r| r.renames.len()).sum();
        write!(
            f,
            "{count} rename operation{} rejected during preparation",
            if count == 1 { "" } else { "s" },
        )
    }
}

impl Error for PreparationError {}

type Partition<R, S, T> = (Vec<R>, Vec<Rejection<S, T>>);

/// Assign each operation its first diagnostic while still detecting later
/// conflicts against the entire batch. This prevents an arbitrary survivor
/// when duplicate-source, duplicate-target and overlap groups intersect.
pub(crate) struct RejectionTracker {
    owners: Vec<Option<usize>>,
    reasons: Vec<RejectionReason>,
}

impl RejectionTracker {
    /// Initializes one unassigned slot per operation. The count is the exact
    /// batch length used by `mark` and `partition`, not an allocation hint.
    pub(crate) fn new(operation_count: usize) -> Self {
        Self {
            owners: vec![None; operation_count],
            reasons: Vec::new(),
        }
    }

    pub(crate) fn mark(
        &mut self,
        indices: impl IntoIterator<Item = usize>,
        reason: impl Into<RejectionReason>,
    ) {
        let group = self.reasons.len();
        let mut assigned = false;
        for index in indices {
            if self.owners[index].is_none() {
                self.owners[index] = Some(group);
                assigned = true;
            }
        }
        if assigned {
            self.reasons.push(reason.into());
        }
    }

    /// Retain each operation's internal state, extracting its original values
    /// only when it is rejected.
    pub(crate) fn partition<R, S, T>(
        self,
        renames: Vec<R>,
        mut into_original: impl FnMut(R) -> Rename<S, T>,
    ) -> Partition<R, S, T> {
        let mut groups: Vec<_> = self
            .reasons
            .into_iter()
            .map(|reason| Rejection {
                renames: Vec::new(),
                reason,
            })
            .collect();
        let mut retained = Vec::new();
        for (rename, owner) in renames.into_iter().zip(self.owners) {
            if let Some(group) = owner {
                groups[group].renames.push(into_original(rename));
            } else {
                retained.push(rename);
            }
        }
        (retained, groups)
    }
}
