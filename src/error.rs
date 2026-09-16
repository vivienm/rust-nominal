use std::{io, path::PathBuf};

use thiserror::Error;

/// An error from strict preparation or plan application.
///
/// Planning and filesystem diagnostics are carried by the rejections in
/// [`crate::PreparationError`], rather than returned directly as this error.
#[derive(Debug, Error)]
#[error(transparent)]
#[non_exhaustive]
pub enum Error {
    /// Rejections from strict preparation.
    Preparation(#[from] crate::PreparationError),
    /// An apply error.
    Apply(#[from] ApplyError),
}

/// A planning diagnostic from [`Renamer::prepare`](crate::Renamer::prepare).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PlanError {
    /// A parent directory could not be resolved while preparing the plan.
    #[error("failed to resolve parent of {path:?}: {source}")]
    ResolvePath {
        /// The original source or target path.
        path: PathBuf,
        /// The underlying filesystem error.
        source: io::Error,
    },
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
    /// A source or target is a strict ancestor of another source or target.
    /// These batches are unsupported because renames can change directory
    /// structure needed by other operations.
    #[error("overlapping batch paths: {ancestor_path:?} contains {descendant_path:?}")]
    OverlappingPaths {
        /// The original path naming an ancestor entry.
        ancestor_path: PathBuf,
        /// The original path naming a descendant entry.
        descendant_path: PathBuf,
    },
    /// The rename operations contain one or more cycles that cannot be
    /// resolved with direct renames alone (e.g. a swap `a <-> b`).
    #[error("rename cycle(s) detected: {cycles:?}")]
    Cycle {
        /// The cycles found in the rename graph. Each inner `Vec` lists the
        /// target paths of one cycle.
        cycles: Vec<Vec<PathBuf>>,
    },
}

/// A filesystem error reported during preparation or
/// [`Plan::reject_conflicts`](crate::Plan::reject_conflicts).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FsError {
    /// The target path is occupied by another directory entry on disk.
    #[error("target {target_path:?} already exists")]
    TargetExists {
        /// The resolved target path captured during preparation.
        target_path: PathBuf,
    },
    /// The source or target could not be inspected. The affected operation is
    /// reported without guessing which path caused a multi-path check to fail.
    #[error("failed to inspect {source_path:?} -> {target_path:?}: {source}")]
    Inspect {
        /// The resolved source path captured during preparation.
        source_path: PathBuf,
        /// The resolved target path captured during preparation.
        target_path: PathBuf,
        /// The underlying filesystem error.
        source: io::Error,
    },
}

/// The error type returned from [`Plan::apply`](crate::plan::Plan::apply).
#[derive(Debug, Error)]
#[error("failed to rename {source_path:?} to {target_path:?}: {source}")]
#[non_exhaustive]
pub struct ApplyError {
    /// The resolved source path captured during preparation.
    pub source_path: PathBuf,
    /// The resolved target path captured during preparation.
    pub target_path: PathBuf,
    /// The underlying rename error.
    pub source: RenameError,
}

/// An error from a single rename operation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RenameError {
    /// The target path already exists.
    #[error("target already exists")]
    TargetExists,
    /// A staged rename failed and the source could not be restored.
    /// The entry is retained at `temporary_path` for manual recovery.
    #[error(
        "rename failed: {source}; restoring the source failed: {recovery_error}; entry retained at {temporary_path:?}"
    )]
    RecoveryFailed {
        /// The absolute temporary path where the entry remains.
        temporary_path: PathBuf,
        /// The failure while moving the staged entry to its destination.
        #[source]
        source: Box<RenameError>,
        /// The failure while restoring the entry to the source path.
        recovery_error: Box<RenameError>,
    },
    /// An I/O error occurred.
    #[error("{0}")]
    Io(#[from] io::Error),
}
