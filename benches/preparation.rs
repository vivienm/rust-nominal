//! Measure preparation with shared or dispersed parents, shuffled inputs and
//! rejections. Run with `cargo bench --bench preparation --all-features`.
//! Fixture creation, input collection and plan destruction are not timed.

#[path = "support/allocations.rs"]
mod allocations;

use std::{fs, hint::black_box, path::PathBuf, time::Instant};

use nominal::{FsError, PlanError, Preparation, RejectionReason, Renamer};

#[derive(Clone, Copy)]
enum Scenario {
    SharedOrdered,
    SharedShuffled,
    SharedMissing,
    SharedSameName,
    DispersedExisting,
    DispersedMissing,
    DispersedSameName,
    DuplicateTargets,
    OccupiedTargets,
    OverlappingSources,
    DuplicateOverlaps,
    Chain,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::SharedOrdered => "shared/existing/ordered",
            Self::SharedShuffled => "shared/existing/shuffled",
            Self::SharedMissing => "shared/missing/shuffled",
            Self::SharedSameName => "shared/same-name/shuffled",
            Self::DispersedExisting => "dispersed/existing/shuffled",
            Self::DispersedMissing => "dispersed/missing/shuffled",
            Self::DispersedSameName => "dispersed/same-name/shuffled",
            Self::DuplicateTargets => "duplicate-targets/shuffled",
            Self::OccupiedTargets => "occupied-targets/shuffled",
            Self::OverlappingSources => "overlapping-sources/shuffled",
            Self::DuplicateOverlaps => "duplicate-overlaps/shuffled",
            Self::Chain => "chain/shuffled",
        }
    }

    fn rejected_count(self, count: usize) -> usize {
        match self {
            Self::DuplicateTargets | Self::OverlappingSources => count / 10 * 2,
            Self::OccupiedTargets => count / 10,
            Self::DuplicateOverlaps => count,
            _ => 0,
        }
    }

    fn check(self, preparation: Preparation<&PathBuf, &PathBuf>, count: usize) {
        let rejections = preparation.rejections();
        assert_eq!(
            rejections
                .iter()
                .map(|rejection| rejection.renames.len())
                .sum::<usize>(),
            self.rejected_count(count),
        );
        for rejection in rejections {
            assert!(matches!(
                (self, &rejection.reason),
                (
                    Self::DuplicateTargets,
                    RejectionReason::Plan(PlanError::DuplicateTarget { .. })
                ) | (
                    Self::OccupiedTargets,
                    RejectionReason::Filesystem(FsError::TargetExists { .. })
                ) | (
                    Self::OverlappingSources,
                    RejectionReason::Plan(PlanError::OverlappingPaths { .. })
                ) | (
                    Self::DuplicateOverlaps,
                    RejectionReason::Plan(
                        PlanError::DuplicateSource { .. } | PlanError::OverlappingPaths { .. }
                    )
                )
            ));
        }
        let (plan, rejections) = preparation.into_parts();
        assert_eq!(plan.len(), count - self.rejected_count(count));
        black_box((&plan, &rejections));
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    pairs: Vec<(PathBuf, PathBuf)>,
}

impl Fixture {
    fn new(count: usize, scenario: Scenario) -> Self {
        assert_eq!(count % 10, 0, "fixture groups contain ten operations");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let incoming = root.join("incoming");
        let library = root.join("library");
        fs::create_dir(&incoming).unwrap();
        fs::create_dir(&library).unwrap();
        if matches!(scenario, Scenario::DuplicateOverlaps) {
            let folder = incoming.join("folder");
            fs::create_dir(&folder).unwrap();
            let mut pairs = Vec::with_capacity(count);
            for index in 0..count / 2 {
                pairs.push((folder.clone(), library.join(format!("folder-{index}"))));
                let child = folder.join(format!("file-{index}"));
                fs::write(&child, []).unwrap();
                pairs.push((child, library.join(format!("file-{index}"))));
            }
            shuffle(&mut pairs);
            return Self { _dir: dir, pairs };
        }
        let dispersed = matches!(
            scenario,
            Scenario::DispersedExisting | Scenario::DispersedMissing | Scenario::DispersedSameName
        );
        let missing = matches!(
            scenario,
            Scenario::SharedMissing | Scenario::DispersedMissing
        );
        let mut pairs = Vec::with_capacity(count);
        for book in 0..count {
            // Dispersed fixtures give every source AND destination its own parent.
            let source_parent = if dispersed {
                let parent = incoming.join(format!("folder-{book:05}"));
                fs::create_dir(&parent).unwrap();
                parent
            } else {
                incoming.clone()
            };
            let source = if matches!(scenario, Scenario::OverlappingSources) && book % 10 < 2 {
                let parent = incoming.join(format!("folder-{:05}", book / 10));
                if book % 10 == 0 {
                    fs::create_dir(&parent).unwrap();
                    parent
                } else {
                    parent.join("child")
                }
            } else {
                source_parent.join(format!("download-{book:05}.epub"))
            };
            if !matches!(scenario, Scenario::OverlappingSources) || book % 10 != 0 {
                fs::write(&source, []).unwrap();
            }
            let target = if matches!(scenario, Scenario::Chain) {
                incoming.join(format!("download-{:05}.epub", book + 1))
            } else {
                let parent = library.join(format!(
                    "Author {:05}",
                    if dispersed { book } else { book / 20 }
                ));
                if !missing {
                    fs::create_dir_all(&parent).unwrap();
                }
                let target_book =
                    if matches!(scenario, Scenario::DuplicateTargets) && book % 10 == 1 {
                        book - 1
                    } else {
                        book
                    };
                if matches!(
                    scenario,
                    Scenario::SharedSameName | Scenario::DispersedSameName
                ) {
                    parent.join(source.file_name().unwrap())
                } else {
                    parent.join(format!("Book {target_book:05}.epub"))
                }
            };
            if matches!(scenario, Scenario::OccupiedTargets) && book % 10 == 0 {
                fs::write(&target, []).unwrap();
            }
            pairs.push((source, target));
        }
        if !matches!(scenario, Scenario::SharedOrdered) {
            shuffle(&mut pairs);
        }
        Self { _dir: dir, pairs }
    }

    fn renamer(&self) -> Renamer<&PathBuf, &PathBuf> {
        Renamer::from_iter(self.pairs.iter().map(|(source, target)| (source, target)))
    }
}

// A fixed-seed Fisher-Yates shuffle keeps runs reproducible without another
// dependency. Fixture generation and shuffling are outside the timed region.
fn shuffle<T>(values: &mut [T]) {
    let mut state = 0x5eeda11ce_u64;
    for i in (1..values.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        values.swap(i, (state % (i as u64 + 1)) as usize);
    }
}

fn measure(count: usize, scenario: Scenario, count_allocations: bool) {
    let fixture = Fixture::new(count, scenario);
    let mut samples = Vec::new();
    for run in 0..8 {
        let renamer = fixture.renamer();
        let start = Instant::now();
        let preparation = black_box(renamer).prepare();
        let elapsed = start.elapsed();
        scenario.check(preparation, count);
        if run != 0 {
            samples.push(elapsed);
        }
    }
    samples.sort();
    print!(
        "{count:5} ops, {scenario:28}: median {median:8.2} ms (min {min:.2}, max {max:.2})",
        scenario = scenario.name(),
        median = samples[samples.len() / 2].as_secs_f64() * 1000.0,
        min = samples[0].as_secs_f64() * 1000.0,
        max = samples[samples.len() - 1].as_secs_f64() * 1000.0,
    );
    if count_allocations {
        // Count separately, excluding input collection, result checks and destruction.
        let renamer = fixture.renamer();
        let (preparation, counts) = allocations::measure(|| black_box(renamer).prepare());
        scenario.check(preparation, count);
        print!(
            ", {} alloc/realloc calls, {} requested bytes",
            counts.calls, counts.bytes
        );
    }
    println!();
}

fn main() {
    let count_allocations = std::env::var_os("NOMINAL_BENCH_ALLOCATIONS").is_some();
    for count in [1_000, 10_000] {
        for scenario in [
            Scenario::SharedOrdered,
            Scenario::SharedShuffled,
            Scenario::SharedMissing,
            Scenario::SharedSameName,
            Scenario::DispersedExisting,
            Scenario::DispersedMissing,
            Scenario::DispersedSameName,
            Scenario::DuplicateTargets,
            Scenario::OccupiedTargets,
            Scenario::OverlappingSources,
            Scenario::DuplicateOverlaps,
            Scenario::Chain,
        ] {
            measure(count, scenario, count_allocations);
        }
    }
}
