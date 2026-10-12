use super::*;

fn ids(s: &Source) -> Vec<UnitId> {
    s.units().collect()
}

#[test]
fn lines_round_trip() {
    for t in ["", "a", "a\n", "a\nb", "a\n\nb\n", "\n\n"] {
        let s = Source::new(t);
        assert_eq!(s.to_text(), t);
        assert!(s.units().all(|u| !s.text(u).is_empty()));
    }
    assert_eq!(ids(&Source::new("a\nb\nc\n")).len(), 3);
}

#[test]
fn edit_in_a_line_names_only_it() {
    let mut s = Source::new("one\ntwo\nthree\n");
    let before = ids(&s);
    let p = s.pos(before[1], 1);
    let q = s.pos(before[1], 2);
    let sp = s.edit(p, q, "W");
    assert_eq!(s.to_text(), "one\ntWo\nthree\n");
    assert_eq!(sp.removed, vec![before[1]]);
    assert_eq!(sp.inserted.len(), 1);
    let after = ids(&s);
    assert_eq!((after[0], after[2]), (before[0], before[2]));
}

#[test]
fn positions_after_an_edit_stay_right() {
    let mut s = Source::new("one\ntwo\nthree\n");
    let u = ids(&s);
    let far = s.pos(u[2], 2);
    assert_eq!(s.line_no(far), 3);
    let a = s.pos(u[0], 3);
    s.edit(a, a, "\nnew line");
    assert_eq!(s.to_text(), "one\nnew line\ntwo\nthree\n");
    assert_eq!(s.line_no(far), 4);
    assert_eq!(s.byte(far), "one\nnew line\ntwo\nth".len());
    assert_eq!(&**s.text(far.unit()), "three\n");
}

#[test]
fn joining_lines_pulls_the_next() {
    let mut s = Source::new("ab\ncd\nef\n");
    let u = ids(&s);
    let a = s.pos(u[0], 2);
    let b = s.pos(u[0], 3);
    let sp = s.edit(a, b, "");
    assert_eq!(s.to_text(), "abcd\nef\n");
    assert_eq!(sp.removed, vec![u[0], u[1]]);
    assert_eq!(sp.inserted.len(), 1);
    assert_eq!(ids(&s)[1], u[2]);
}

#[test]
fn split_and_cross_unit_edits() {
    let mut s = Source::new("ab\ncd\nef\ngh\n");
    let u = ids(&s);
    let a = s.pos(u[1], 1);
    let b = s.pos(u[2], 1);
    let sp = s.edit(a, b, "X\nY\nZ");
    assert_eq!(s.to_text(), "ab\ncX\nY\nZf\ngh\n");
    assert_eq!(sp.removed, vec![u[1], u[2]]);
    assert_eq!(sp.inserted.len(), 3);
    let v = ids(&s);
    assert_eq!((v[0], v[4]), (u[0], u[3]));
    assert!(!s.contains(u[1]));
}

#[test]
fn unchanged_text_keeps_ids() {
    let mut s = Source::new("a\nb\nc\n");
    let u = ids(&s);
    let sp = s.replace("a\nB\nc\n");
    assert_eq!(sp.removed, vec![u[1]]);
    assert_eq!((ids(&s)[0], ids(&s)[2]), (u[0], u[2]));
    let sp = s.replace("a\nB\nc\n");
    assert_eq!(sp, Splice::default());
}

#[test]
fn edits_at_the_ends() {
    let mut s = Source::new("a\nb");
    let u = ids(&s);
    let end = s.pos(u[1], 1);
    s.edit(end, end, "c\n");
    assert_eq!(s.to_text(), "a\nbc\n");
    let st = s.start().unwrap();
    s.edit(st, st, "z\n");
    assert_eq!(s.to_text(), "z\na\nbc\n");
    let mut e = Source::new("");
    let sp = e.replace("x\ny");
    assert_eq!((e.to_text().as_str(), sp.inserted.len()), ("x\ny", 2));
}

#[test]
fn at_byte_and_cursor() {
    let s = Source::new("ab\ncd\n");
    let p = s.at_byte(4).unwrap();
    assert_eq!(s.byte(p), 4);
    let mut c = Cursor::new(&s, s.start().unwrap());
    let mut got = String::new();
    loop {
        got.push_str(c.rest());
        let n = c.rest().len();
        c.bump(n);
        if !c.next_unit(&s) {
            break;
        }
    }
    assert_eq!(got, "ab\ncd\n");
    assert_eq!(
        s.slice(s.at_byte(1).unwrap(), s.at_byte(5).unwrap()),
        "b\ncd"
    );
}

/// Random edits: the text is the string edited the same way, and ids of
/// units not named by a splice survive.
#[test]
fn random_edits() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        usize::try_from(seed % n.max(1) as u64).unwrap()
    };
    let alphabet = ["a", "b", "\n", "\n\n", "xy"];
    let mut model = String::from("one\ntwo\n\nthree");
    let mut s = Source::new(&model);
    for _ in 0..5000 {
        let len = model.len();
        let x = rnd(len + 1);
        let y = (x + rnd(4)).min(len);
        let mut new = String::new();
        for _ in 0..rnd(3) {
            new.push_str(alphabet[rnd(alphabet.len())]);
        }
        let before = ids(&s);
        let (Some(a), Some(b)) = (s.at_byte(x), s.at_byte(y)) else {
            model = new.clone();
            s.replace(&new);
            continue;
        };
        let sp = s.edit(a, b, &new);
        model.replace_range(x..y, &new);
        assert_eq!(s.to_text(), model);
        let kept: Vec<_> = before.iter().filter(|u| !sp.removed.contains(u)).collect();
        assert!(kept.iter().all(|u| s.contains(**u)));
        assert_eq!(ids(&s).len(), kept.len() + sp.inserted.len());
        for u in s.units() {
            assert!(!s.text(u).is_empty());
        }
    }
}
