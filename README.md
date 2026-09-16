# nominal

A Rust library for batch file renaming.

## Prepare once, choose how to handle rejections

Strict callers obtain a plan only if the whole batch passes preparation:

```rust
use nominal::Renamer;

fn main() -> Result<(), nominal::Error> {
    let plan = Renamer::from_iter([("old.txt", "new.txt")])
        .prepare()
        .into_plan()?;
    plan.apply()?;
    Ok(())
}
```

`Plan::try_from(preparation)?` and `preparation.try_into()?` provide the same
strict conversion as `preparation.into_plan()?`.

Best-effort callers can report rejected operations and apply the retained plan:

```rust
use nominal::Renamer;

let (plan, rejections) = Renamer::from_iter([("old.txt", "new.txt")])
    .prepare()
    .into_parts();
for rejection in rejections {
    for rename in rejection.renames {
        eprintln!("{}: {}", rename, rejection.reason);
    }
}
for result in plan.apply_iter() {
    // Each result describes a completed rename or an application error.
    println!("{result:?}");
}
```

Preparation does not modify the filesystem. It removes no-ops, resolves parent
aliases, rejects duplicate contenders, overlaps and cycles, orders dependencies,
and checks occupied targets. Rejections include the affected operations and any
filesystem inspection error. Each rejected operation is counted once, even if it
has several problems. `rejections().len()` counts diagnostic groups; sum their
`renames.len()` values when you need the number of rejected operations.

`PreparationError` implements `std::error::Error` and owns the rejected paths,
so it can be propagated with `?` even when the input paths were borrowed. Its
`Display` is a single-line summary; applications can iterate over its
`rejections` to log each affected operation with its diagnostic.

`Plan::reject_conflicts()` explicitly removes new conflicts if another check is
needed after preparation. `apply()` stops at the first error; `apply_iter()` lets
the caller continue with independent operations.

`Plan::iter()` yields views of the retained operations in execution order for
inspection or custom output. For `Plan<S, T>`, each view is a
`Rename<PlannedPath<'_, S>, PlannedPath<'_, T>>`. Each endpoint exposes `original`
(a reference to the input value, including its metadata) and `resolved` (the
captured execution path). Its `AsRef<Path>` uses `resolved`, so displaying the
view shows the paths the plan will use, even if interior mutability changes the
originals. `Plan::write_to()` and `write_colored_to()` also use these captured
paths. Creating views does not allocate or clone paths, execute renames, or
consume the plan.

## Filesystem guarantees

- System renames atomically refuse destination replacement on Linux, Android,
  Apple platforms and Windows, including temporary staging and recovery. If the
  platform or filesystem lacks the primitive, the rename fails without falling
  back to an overwriting operation.
- Case-only changes can temporarily move the entry to a sibling name. Failed
  restoration reports the retained temporary path; it does not delete the data.
- A batch is not a transaction. Completed renames are not rolled back, and source
  paths and parent directories are not locked against concurrent changes.
- Preparation does not guarantee source existence, permissions or same-volume
  destinations. Application can still fail. Missing destination parents are
  created during application; cross-filesystem copying is not supported.
- Symlinks themselves can be renamed. Paths are unrestricted: directory
  confinement, when needed, is the caller's responsibility.
- Missing names are compared by spelling. Case aliases between missing names
  may only be detected during application; existing targets remain protected.

## Optional features

- `ansi`: colored plan output using `LS_COLORS`.
- `confirm`: interactive confirmation.
- `unicode`: natural Unicode ordering, with a lexical tie-break for equal
  collation results.

## Migrating from 0.1

Replace `renamer.plan()?` and its initial `check_fs()` call with
`renamer.prepare().into_plan()?` for strict handling, or
`renamer.prepare().into_parts()` for best-effort handling. Preparation includes
filesystem checks, and `PreparationError` owns all rejected paths. The former
`FsConflict` diagnostics are now `FsError`, including inspection I/O errors.
Use `reject_conflicts()` only when an existing plan needs to be checked again.

`Error` has two variants: `Preparation` and `Apply`. The former `Error::Plan`
and `Error::Filesystem` variants and their `From` conversions have been removed.
Inspect `PreparationError::rejections` and `RejectionReason` for the individual
`PlanError` and `FsError` diagnostics.

## Preparation benchmark

Run `cargo bench --bench preparation --all-features` to measure batches of
1,000 and 10,000 existing files, grouped into author directories with twenty
books each. Both existing and missing destination directories are covered.
The benchmark reports seven samples after a warm-up, excluding fixture setup,
input collection and plan destruction; it does not execute the renames.
Results depend on the filesystem and measure preparation with warm caches.

[API documentation](https://vivienm.github.io/rust-nominal/docs/nominal/)
