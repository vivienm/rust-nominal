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

[API documentation](https://vivienm.github.io/rust-nominal/docs/nominal/)
