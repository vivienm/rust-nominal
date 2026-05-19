use std::{io, path::PathBuf};

use thiserror::Error;

/// The general error type for this crate.
#[derive(Debug, Error)]
#[error(transparent)]
#[non_exhaustive]
pub enum Error {
    /// A plan error.
    Plan(#[from] PlanError),
    /// An apply error.
    Apply(#[from] ApplyError),
}

/// The error type returned from
/// [`Renamer::plan`](crate::renamer::Renamer::plan).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PlanError {
    /// An ICU error occurred while preparing the collator.
    #[cfg(feature = "unicode")]
    #[error("ICU error: {0}")]
    Icu(#[from] icu_provider::DataError),
    /// Multiple rename operations share the same source path.
    #[error("multiple targets map from source {path:?}")]
    DuplicateSource {
        /// The source path that has more than one target.
        path: PathBuf,
    },
    /// Multiple rename operations share the same target path.
    #[error("multiple sources map to target {path:?}")]
    DuplicateTarget {
        /// The target path that has more than one source.
        path: PathBuf,
    },
    /// The rename operations form a cycle that cannot be resolved with direct
    /// renames alone (e.g. a swap `a <-> b`).
    #[error("rename cycle detected: {targets:?}")]
    Cycle {
        /// The target paths involved in the cycle.
        targets: Vec<PathBuf>,
    },
}

/// The error type returned from [`Plan::apply`](crate::plan::Plan::apply).
#[derive(Debug, Error)]
#[error("failed to rename {source_path:?} to {target_path:?}: {source}")]
#[non_exhaustive]
pub struct ApplyError {
    /// The source path of the rename operation.
    pub source_path: PathBuf,
    /// The target path of the rename operation.
    pub target_path: PathBuf,
    /// The underlying rename error.
    pub source: RenameError,
    /// The number of rename operations successfully applied before this
    /// failure.
    pub applied: usize,
}

/// An error from a single rename operation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RenameError {
    /// The target path already exists.
    #[error("target already exists")]
    TargetExists,
    /// An I/O error occurred.
    #[error("{0}")]
    Io(#[from] io::Error),
}
