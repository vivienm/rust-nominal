use std::{fmt, io, path::PathBuf};

use thiserror::Error;

/// The general error type for this crate.
#[derive(Debug, Error)]
#[error(transparent)]
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
    #[cfg(feature = "unicode")]
    /// The ICU collator could not be created.
    #[error("could not create collator: {0}")]
    IcuCollator(#[from] icu_collator::Error),
    /// Multiple rename operations share the same source path.
    #[error("multiple targets map from source {source_path:?}")]
    DuplicateSource {
        /// The source path that has more than one target.
        source_path: PathBuf,
    },
    /// Multiple rename operations share the same target path.
    #[error("multiple sources map to target {target:?}")]
    DuplicateTarget {
        /// The target path that has more than one source.
        target: PathBuf,
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
#[derive(Debug)]
pub struct ApplyError {
    /// The source path of the rename operation.
    pub source: PathBuf,
    /// The target path of the rename operation.
    pub target: PathBuf,
    /// The details of the error.
    pub details: ApplyErrorDetails,
}

/// The details of an [`ApplyError`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ApplyErrorDetails {
    /// The target path already exists.
    TargetExists,
    /// An I/O error occurred.
    Io(io::Error),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to rename {:?} to {:?}: {}",
            self.source, self.target, self.details
        )
    }
}

impl fmt::Display for ApplyErrorDetails {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyErrorDetails::TargetExists => write!(f, "target already exists"),
            ApplyErrorDetails::Io(err) => write!(f, "{}", err),
        }
    }
}

impl std::error::Error for ApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.details {
            ApplyErrorDetails::TargetExists => None,
            ApplyErrorDetails::Io(err) => Some(err),
        }
    }
}

impl ApplyError {
    pub(crate) fn new(
        source: impl Into<PathBuf>,
        target: impl Into<PathBuf>,
        details: ApplyErrorDetails,
    ) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            details,
        }
    }

    pub(crate) fn target_exists(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self::new(source, target, ApplyErrorDetails::TargetExists)
    }

    pub(crate) fn from_io(
        from: impl Into<PathBuf>,
        to: impl Into<PathBuf>,
        source: io::Error,
    ) -> Self {
        Self::new(from, to, ApplyErrorDetails::Io(source))
    }
}
