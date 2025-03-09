//! A Rust library for batch file renaming.
//!
//! [Repository]
//!
//! [Repository]: https://github.com/vivienm/rust-nominal
//!
//! # Examples
//!
//! ```
//! # use std::fs::File;
//! # use nominal::{Plan, Renamer};
//! let temp_dir = tempfile::tempdir()?;
//! let old_path = temp_dir.path().join("old.txt");
//! let new_path = temp_dir.path().join("new.txt");
//!
//! File::create(&old_path)?;
//! assert!(old_path.exists());
//! assert!(!new_path.exists());
//!
//! let mut renamer = Renamer::new();
//! renamer.add(&old_path, &new_path);
//!
//! let plan = renamer.plan()?;
//! plan.apply()?;
//!
//! assert!(!old_path.exists());
//! assert!(new_path.exists());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod error;
mod fsutil;
mod operation;
mod plan;
mod renamer;

pub use self::{
    error::{ApplyError, ApplyErrorDetails, Error, PlanError},
    plan::Plan,
    renamer::Renamer,
};
