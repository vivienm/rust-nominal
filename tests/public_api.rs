//! Verifies that the public iterator and its result can be named by consumers.

use nominal::{ApplyIter, Rename, Renamer};

fn apply_renames<'s, 't>(iter: ApplyIter<&'s str, &'t str>) -> Vec<Rename<&'s str, &'t str>> {
    iter.map(Result::unwrap).collect()
}

#[test]
fn iterator_types_are_publicly_accessible() {
    let plan = Renamer::new().plan().unwrap();
    assert!(apply_renames(plan.apply_iter()).is_empty());
    let rename = Rename::new("source", "target");
    assert_eq!(<(&str, &str)>::from(rename), ("source", "target"));
}
