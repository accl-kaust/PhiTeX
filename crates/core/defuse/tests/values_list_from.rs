//! `Values::list_from` interns a borrowed slice as `Values::list` does:
//! the same id, the same elements, and a second call adds nothing.

use phitex_defuse::Values;

#[test]
fn list_from_matches_list() {
    let v = Values::new();
    let xs = vec![v.int(1), v.str("a"), v.int(3)];
    let a = v.list_from(&xs);
    let n = v.len();
    let b = v.list(xs.clone());
    assert_eq!(a, b);
    assert_eq!(v.len(), n);
    assert_eq!(v.list_from(&xs), a);
    assert_eq!(v.with_list(a, <[_]>::to_vec), Some(xs));
    assert_eq!(v.list_from(&[]), v.list(Vec::new()));
}
