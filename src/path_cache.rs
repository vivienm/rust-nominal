//! Resolve paths, identify entries, and transfer validated endpoints into a plan.

use std::{
    collections::HashMap,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use crate::{
    PlanError, Rename,
    fsutil::{EntryKey, IdentificationError, IdentifiedPath, ResolvedPath},
    plan::{PreparedPath, PreparedRename},
    preparation::Rejection,
};

/// Identification failures retain the resolved path for overlap diagnostics.
#[derive(Debug, thiserror::Error)]
enum PathFailure {
    #[error(transparent)]
    Resolve(io::Error),
    #[error(transparent)]
    Identify(#[from] IdentificationError),
}

impl PathFailure {
    fn resolved(&self) -> Option<&Path> {
        match self {
            Self::Resolve(_) => None,
            Self::Identify(error) => Some(error.resolved.as_path()),
        }
    }

    fn to_plan_error(self: &Arc<Self>, path: impl Into<PathBuf>) -> PlanError {
        let error = match self.as_ref() {
            Self::Resolve(error) => error,
            Self::Identify(error) => &error.source,
        };
        // Several operations may share a failed endpoint. Preserve native error
        // codes, or share the original error and its source chain otherwise.
        let source = match error.raw_os_error() {
            Some(code) => io::Error::from_raw_os_error(code),
            None => io::Error::new(error.kind(), Arc::clone(self)),
        };
        PlanError::ResolvePath {
            path: path.into(),
            source,
        }
    }
}

/// First phase: owned resolution results, with no shared mutable state.
pub(crate) struct ResolutionCache {
    entries: HashMap<OsString, io::Result<ResolvedPath>>,
}

impl ResolutionCache {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
        }
    }

    pub(crate) fn resolve(&mut self, path: &Path) {
        if !self.entries.contains_key(path.as_os_str()) {
            self.entries
                .insert(path.as_os_str().to_os_string(), ResolvedPath::new(path));
        }
    }

    pub(crate) fn resolved(&self, path: &Path) -> Option<&Path> {
        self.entries
            .get(path.as_os_str())
            .and_then(|result| result.as_ref().ok())
            .map(ResolvedPath::as_path)
    }

    /// Consume resolution results before sharing entry identities.
    /// Identification remains lazy so discarded no-ops need no entry metadata.
    pub(crate) fn into_identification(self) -> IdentificationCache {
        let identified = HashMap::with_capacity(self.entries.len());
        IdentificationCache {
            pending: self.entries,
            identified,
        }
    }
}

type Identification = Result<Rc<IdentifiedPath>, Arc<PathFailure>>;

/// Second phase: consume resolved paths and share only immutable results.
pub(crate) struct IdentificationCache {
    pending: HashMap<OsString, io::Result<ResolvedPath>>,
    identified: HashMap<OsString, Identification>,
}

impl IdentificationCache {
    pub(crate) fn inspect<P: AsRef<Path>>(&mut self, original: P) -> InspectedPath<P> {
        let path = original.as_ref();
        let identification = if let Some(identified) = self.identified.get(path.as_os_str()) {
            identified.clone()
        } else {
            // A cache miss is valid too: resolve the path before identifying it.
            // Correctness never depends on a previous lookup or registration.
            let (spelling, resolved) = self
                .pending
                .remove_entry(path.as_os_str())
                .unwrap_or_else(|| (path.as_os_str().to_os_string(), ResolvedPath::new(path)));
            let identified = resolved
                .map_err(PathFailure::Resolve)
                .and_then(|resolved| IdentifiedPath::new(resolved).map_err(PathFailure::from))
                .map(Rc::new)
                .map_err(Arc::new);
            self.identified.insert(spelling, identified.clone());
            identified
        };
        InspectedPath {
            original,
            identification,
        }
    }
}

/// An endpoint with either complete identity data or a diagnostic result.
pub(crate) struct InspectedPath<P> {
    original: P,
    identification: Identification,
}

impl<P> InspectedPath<P> {
    pub(crate) fn original(&self) -> &P {
        &self.original
    }

    pub(crate) fn resolved(&self) -> Option<&Path> {
        match &self.identification {
            Ok(path) => Some(path.as_path()),
            Err(failure) => failure.resolved(),
        }
    }

    pub(crate) fn key(&self) -> Option<&EntryKey> {
        self.identification.as_ref().ok().map(|path| path.key())
    }
}

pub(crate) type InspectedRename<S, T> = Rename<InspectedPath<S>, InspectedPath<T>>;

impl<S, T> InspectedRename<S, T> {
    pub(crate) fn into_original(self) -> Rename<S, T> {
        Rename::new(self.source.original, self.target.original)
    }
}

impl<S: AsRef<Path>, T: AsRef<Path>> InspectedRename<S, T> {
    /// Prefer the source diagnostic when both endpoints failed inspection.
    pub(crate) fn plan_error(&self) -> Option<PlanError> {
        let source = self.source.identification.as_ref().err();
        let target = self.target.identification.as_ref().err();
        match (source, target) {
            (Some(source), _) => Some(source.to_plan_error(self.source.original.as_ref())),
            (None, Some(target)) => Some(target.to_plan_error(self.target.original.as_ref())),
            (None, None) => None,
        }
    }

    /// Only successful endpoint results can produce a prepared operation.
    pub(crate) fn into_prepared(self) -> Result<PreparedRename<S, T>, Rejection<S, T>> {
        let source = self.source;
        let target = self.target;
        let reason = match (source.identification, target.identification) {
            (Ok(source_path), Ok(target_path)) => {
                // Move the final owner's data; other endpoints clone the complete pair.
                return Ok(Rename::new(
                    PreparedPath::new(source.original, Rc::unwrap_or_clone(source_path)),
                    PreparedPath::new(target.original, Rc::unwrap_or_clone(target_path)),
                ));
            }
            (Err(error), _) => error.to_plan_error(source.original.as_ref()),
            (_, Err(error)) => error.to_plan_error(target.original.as_ref()),
        };
        Err(Rejection {
            renames: vec![Rename::new(source.original, target.original)],
            reason: reason.into(),
        })
    }
}
