//! Measure preparation of Bookworm-like batches with shared parent directories.
//!
//! Run with `cargo bench --bench preparation --all-features`.
//! Fixture creation, input collection and plan destruction are not timed.

use std::{fs, hint::black_box, path::PathBuf, time::Instant};

use nominal::Renamer;

fn measure(count: usize, existing_targets: bool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let incoming = root.join("incoming");
    let library = root.join("library");
    fs::create_dir(&incoming).unwrap();
    fs::create_dir(&library).unwrap();
    // Twenty books per author, all sharing the same destination parent.
    let targets: Vec<PathBuf> = (0..count.div_ceil(20))
        .map(|author| library.join(format!("Author {author:05}")))
        .collect();
    if existing_targets {
        for target in &targets {
            fs::create_dir(target).unwrap();
        }
    }
    let pairs: Vec<_> = (0..count)
        .map(|book| {
            let source = incoming.join(format!("download-{book:05}.epub"));
            fs::write(&source, []).unwrap();
            let target = targets[book / 20].join(format!("Book {book:05}.epub"));
            (source, target)
        })
        .collect();

    let mut samples = Vec::new();
    for run in 0..8 {
        let renamer = Renamer::from_iter(pairs.iter().map(|(source, target)| (source, target)));
        let start = Instant::now();
        let preparation = black_box(renamer).prepare();
        let elapsed = start.elapsed();
        let plan = preparation.into_plan().unwrap();
        assert_eq!(plan.len(), count);
        black_box(&plan);
        // Warm the filesystem caches before collecting seven samples.
        if run != 0 {
            samples.push(elapsed);
        }
    }
    samples.sort();
    println!(
        "{count:5} files, {parents:8} destination parents: median {median:8.2} ms (min {min:.2}, max {max:.2})",
        parents = if existing_targets {
            "existing"
        } else {
            "missing"
        },
        median = samples[samples.len() / 2].as_secs_f64() * 1000.0,
        min = samples[0].as_secs_f64() * 1000.0,
        max = samples[samples.len() - 1].as_secs_f64() * 1000.0,
    );
}

fn main() {
    for count in [1_000, 10_000] {
        for existing_targets in [true, false] {
            measure(count, existing_targets);
        }
    }
}
