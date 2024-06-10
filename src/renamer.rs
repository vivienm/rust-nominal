use std::path::Path;

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

        // Sort the renames by target path.
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

                let p1: Vec<u16> = p1.as_os_str().collect();
                let p2: Vec<u16> = p2.as_os_str().collect();
                collator.compare_utf16(&p1, &p2)
            }

            renames
                .sort_by(|r1, r2| compare_paths(&collator, r1.target.as_ref(), r2.target.as_ref()));
        }
        #[cfg(not(feature = "unicode"))]
        {
            renames.sort_by(|r1, r2| r1.target.as_ref().cmp(r2.target.as_ref()));
        }

        Ok(Plan { renames })
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
