//! Verifies that the public iterator and its result can be named by consumers.

use nominal::{ApplyIter, Plan, PlannedPath, Rename, Renamer};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
};

#[derive(Debug)]
struct SwitchingPath {
    paths: [PathBuf; 2],
    selected: Cell<usize>,
}

impl SwitchingPath {
    fn new(initial: PathBuf, alternate: PathBuf) -> Self {
        Self {
            paths: [initial, alternate],
            selected: Cell::new(0),
        }
    }
}

impl AsRef<Path> for SwitchingPath {
    fn as_ref(&self) -> &Path {
        &self.paths[self.selected.get()]
    }
}

fn inspect_renames<S, T>(plan: &Plan<S, T>) -> Vec<Rename<PlannedPath<'_, S>, PlannedPath<'_, T>>> {
    plan.iter().collect()
}

fn apply_renames<S, T>(iter: ApplyIter<S, T>) -> Vec<Rename<S, T>> {
    iter.map(Result::unwrap).collect()
}

// Consumers of prepared plans need no path-conversion bounds on their payloads.
fn check_and_preview<S, T>(plan: &mut Plan<S, T>) -> Vec<u8> {
    assert!(plan.reject_conflicts().is_empty());
    let mut output = Vec::new();
    plan.write_to(&mut output).unwrap();
    #[cfg(feature = "ansi")]
    {
        let mut colored = Vec::new();
        plan.write_colored_to(&mut colored, &lscolors::LsColors::empty())
            .unwrap();
        assert_eq!(colored, output);
    }
    output
}

fn apply_plan<S, T>(plan: Plan<S, T>) {
    plan.apply().unwrap();
}

fn execute_plan<S, T>(plan: Plan<S, T>) -> Vec<Rename<S, T>> {
    apply_renames(plan.apply_iter())
}

#[cfg(feature = "confirm")]
fn confirm_empty_plan<S, T>(plan: &Plan<S, T>) {
    assert!(plan.is_empty());
    assert_eq!(plan.confirm().unwrap(), None);
}

#[test]
fn prepared_plan_methods_need_no_path_conversion_bounds() {
    let empty_plan = || Renamer::<&str, &str>::new().prepare().into_plan().unwrap();
    let mut plan = empty_plan();
    assert!(check_and_preview(&mut plan).is_empty());
    #[cfg(feature = "confirm")]
    confirm_empty_plan(&plan);
    apply_plan(plan);
    assert!(execute_plan(empty_plan()).is_empty());
}

#[test]
fn iterator_types_are_publicly_accessible() {
    // Temporary sharing during preparation must not leak Rc into these types.
    fn assert_send_sync<T: Send + Sync>(_: &T) {}

    let plan = Renamer::<&str, &str>::new().prepare().into_plan().unwrap();
    assert_send_sync(&plan);
    assert_eq!(plan.iter().len(), 0);
    let iter = plan.apply_iter();
    assert_send_sync(&iter);
    assert!(apply_renames(iter).is_empty());
    let rename = Rename::new("source", "target");
    assert_eq!(<(&str, &str)>::from(rename), ("source", "target"));
}

#[test]
fn plan_iteration_borrows_original_paths_in_execution_order_without_renaming() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    let alias_b = dir.path().join("./b");
    let c = dir.path().join("c");
    std::fs::write(&a, "A").unwrap();
    std::fs::write(&b, "B").unwrap();
    let plan = Renamer::from_iter([(&a, &b), (&alias_b, &c)])
        .prepare()
        .into_plan()
        .unwrap();

    let inspected: Vec<_> = inspect_renames(&plan)
        .into_iter()
        .map(|r| (*r.source.original, *r.target.original))
        .collect();
    assert_eq!(inspected, [(&alias_b, &c), (&a, &b)]);
    assert!(std::ptr::eq(inspected[0].0, &alias_b));
    assert_eq!(inspected[0].0.as_os_str(), alias_b.as_os_str());
    let root = dir.path().canonicalize().unwrap();
    let first = plan.iter().next().unwrap();
    assert_eq!(
        first.source.resolved.as_os_str(),
        root.join("b").as_os_str()
    );
    assert_eq!(first.target.resolved, root.join("c"));
    assert!(std::ptr::eq(
        first.source.resolved,
        plan.iter().next().unwrap().source.resolved,
    ));
    assert_eq!(plan.iter().len(), 2);
    assert_eq!(*plan.iter().next_back().unwrap().source.original, &a);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "A");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "B");
    assert!(!c.exists());

    let applied: Vec<_> = plan
        .apply_iter()
        .map(|result| {
            let rename = result.unwrap();
            (rename.source, rename.target)
        })
        .collect();
    assert_eq!(applied, inspected);
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "A");
    assert_eq!(std::fs::read_to_string(&c).unwrap(), "B");
}

#[test]
fn preparation_supports_try_from_and_try_into_without_executing() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    std::fs::write(&source, "contents").unwrap();
    let prepare = || Renamer::from_iter([(&source, &target)]).prepare();

    let from = nominal::Plan::try_from(prepare()).unwrap();
    let into: nominal::Plan<&std::path::PathBuf, &std::path::PathBuf> =
        prepare().try_into().unwrap();
    assert_eq!(from.len(), 1);
    assert_eq!(into.len(), 1);
    assert!(source.exists());
    assert!(!target.exists());

    std::fs::write(&target, "occupied").unwrap();
    let from_error = nominal::Plan::try_from(prepare()).unwrap_err();
    let into_result: Result<nominal::Plan<_, _>, nominal::PreparationError> = prepare().try_into();
    let into_error = into_result.unwrap_err();
    assert_eq!(from_error.to_string(), into_error.to_string());
    assert_eq!(
        from_error.to_string(),
        prepare().into_plan().unwrap_err().to_string()
    );
    assert_eq!(
        from_error.to_string(),
        "1 rename operation rejected during preparation"
    );
    assert_eq!(from_error.rejections[0].renames[0].source, source);
    // Strict errors own their paths and support ordinary boxed error propagation.
    let _: Box<dyn std::error::Error + Send + Sync> = from_error.into();
    assert_eq!(std::fs::read_to_string(target).unwrap(), "occupied");
}

#[test]
fn execution_uses_captured_paths_after_original_values_change() {
    for (switch_source, switch_target) in [(true, false), (false, true), (true, true)] {
        let dir = tempfile::tempdir().unwrap();
        // Exact spellings exercise the former optimization that reused originals.
        let root = dir.path().canonicalize().unwrap();
        let source = root.join("source");
        let target = root.join("target");
        let unrelated = root.join("unrelated");
        let alternate_target = root.join("alternate-target");
        std::fs::write(&source, "planned contents").unwrap();
        std::fs::write(&unrelated, "unrelated contents").unwrap();
        let mut plan = Renamer::from_iter([(
            SwitchingPath::new(source.clone(), unrelated.clone()),
            SwitchingPath::new(target.clone(), alternate_target.clone()),
        )])
        .prepare()
        .into_plan()
        .unwrap();
        let rename = plan.iter().next().unwrap();
        if switch_source {
            rename.source.original.selected.set(1);
        }
        if switch_target {
            rename.target.original.selected.set(1);
        }
        // The view remains cheap to copy even when the payload is not Clone.
        let endpoint = rename.source;
        let copied = endpoint;
        assert!(std::ptr::eq(endpoint.original, copied.original));
        assert_eq!(rename.source.resolved, source);
        assert_eq!(rename.target.resolved, target);
        assert_eq!(rename.source.as_ref(), source);
        assert_eq!(rename.target.as_ref(), target);
        let expected = format!("{}\n", Rename::new(&source, &target));
        assert_eq!(format!("{rename}\n"), expected);
        assert_eq!(check_and_preview(&mut plan), expected.as_bytes());
        let applied = plan.apply_iter().next().unwrap().unwrap();
        assert_eq!(applied.source.selected.get(), usize::from(switch_source));
        assert_eq!(applied.target.selected.get(), usize::from(switch_target));
        assert!(!source.exists());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "planned contents"
        );
        assert_eq!(
            std::fs::read_to_string(&unrelated).unwrap(),
            "unrelated contents"
        );
        assert!(!alternate_target.exists());
    }
}

#[test]
fn recheck_uses_captured_target_after_original_value_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let source = root.join("source");
    let target = root.join("target");
    let alternate_target = root.join("alternate-target");
    std::fs::write(&source, "source contents").unwrap();
    let mut plan = Renamer::from_iter([(
        &source,
        SwitchingPath::new(target.clone(), alternate_target.clone()),
    )])
    .prepare()
    .into_plan()
    .unwrap();
    plan.iter().next().unwrap().target.original.selected.set(1);
    std::fs::write(&target, "new occupant").unwrap();

    let rejected = plan.reject_conflicts();
    assert_eq!(rejected.len(), 1);
    assert!(plan.is_empty());
    assert_eq!(rejected[0].renames[0].target.selected.get(), 1);
    assert!(matches!(
        &rejected[0].reason,
        nominal::RejectionReason::Filesystem(nominal::FsError::TargetExists { target_path })
            if target_path == &target
    ));
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "source contents");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new occupant");
    assert!(!alternate_target.exists());
}

#[test]
fn apply_errors_report_captured_paths_after_original_values_change() {
    for occupied_target in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let source = root.join("source");
        let target = root.join("target");
        let unrelated = root.join("unrelated");
        let alternate_target = root.join("alternate-target");
        std::fs::write(&source, "source contents").unwrap();
        std::fs::write(&unrelated, "unrelated contents").unwrap();
        let plan = Renamer::from_iter([(
            SwitchingPath::new(source.clone(), unrelated.clone()),
            SwitchingPath::new(target.clone(), alternate_target.clone()),
        )])
        .prepare()
        .into_plan()
        .unwrap();
        let rename = plan.iter().next().unwrap();
        rename.source.original.selected.set(1);
        rename.target.original.selected.set(1);
        if occupied_target {
            std::fs::write(&target, "new occupant").unwrap();
        } else {
            std::fs::remove_file(&source).unwrap();
        }

        let error = plan.apply().unwrap_err();
        assert_eq!(error.source_path, source);
        assert_eq!(error.target_path, target);
        if occupied_target {
            assert!(matches!(error.source, nominal::RenameError::TargetExists));
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "new occupant");
            assert_eq!(std::fs::read_to_string(&source).unwrap(), "source contents");
        } else {
            assert!(matches!(error.source, nominal::RenameError::Io(error)
                if error.kind() == std::io::ErrorKind::NotFound));
            assert!(!target.exists());
        }
        assert_eq!(
            std::fs::read_to_string(&unrelated).unwrap(),
            "unrelated contents"
        );
        assert!(!alternate_target.exists());
    }
}

#[cfg(unix)]
#[test]
fn inspection_errors_report_captured_paths_after_original_values_change() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let source = root.join("source");
    let target_parent = root.join("parent");
    let target = target_parent.join("target");
    std::fs::write(&source, "source contents").unwrap();
    std::fs::create_dir(&target_parent).unwrap();
    let mut plan = Renamer::from_iter([(
        SwitchingPath::new(source.clone(), root.join("alternate-source")),
        SwitchingPath::new(target.clone(), root.join("alternate-target")),
    )])
    .prepare()
    .into_plan()
    .unwrap();
    let rename = plan.iter().next().unwrap();
    rename.source.original.selected.set(1);
    rename.target.original.selected.set(1);
    std::fs::remove_dir(&target_parent).unwrap();
    std::fs::write(&target_parent, "blocking file").unwrap();

    let rejected = plan.reject_conflicts();
    assert_eq!(rejected.len(), 1);
    assert!(plan.is_empty());
    assert!(matches!(
        &rejected[0].reason,
        nominal::RejectionReason::Filesystem(nominal::FsError::Inspect {
            source_path, target_path, source: error,
        }) if source_path == &source && target_path == &target
            && error.kind() == std::io::ErrorKind::NotADirectory
    ));
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "source contents");
}
