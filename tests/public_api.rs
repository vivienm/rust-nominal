//! Verifies that the public iterator and its result can be named by consumers.

use nominal::{ApplyIter, Rename, Renamer};

fn apply_renames<'s, 't>(iter: ApplyIter<&'s str, &'t str>) -> Vec<Rename<&'s str, &'t str>> {
    iter.map(Result::unwrap).collect()
}

#[test]
fn iterator_types_are_publicly_accessible() {
    let plan = Renamer::new().prepare().into_plan().unwrap();
    assert!(apply_renames(plan.apply_iter()).is_empty());
    let rename = Rename::new("source", "target");
    assert_eq!(<(&str, &str)>::from(rename), ("source", "target"));
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
    assert_eq!(from_error.rejections[0].renames[0].source, source);
    assert_eq!(std::fs::read_to_string(target).unwrap(), "occupied");
}
