//! Knuth's trap test for METAFONT (`triptrap/`, as web2c's
//! `mftraptest.test` runs it), compared byte for byte with what TeX Live
//! 2026's own `mf` wrote in the same run (tests/bless.sh, the date fixed
//! by `SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1`): the terminal
//! output and log of both passes, the TFM file and the GF file.

use std::collections::HashMap;
use std::path::PathBuf;

use phitex_mf::{Config, FileKind, Io, Mf};

fn fixture(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/trap")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The trap directory: `trap.mf`, and `trap.base` once dumped.
struct TrapIo(HashMap<Vec<u8>, Vec<u8>>);

impl Io for TrapIo {
    fn open_in(&mut self, name: &[u8], kind: FileKind) -> Option<(Vec<u8>, Vec<u8>)> {
        let mut tries = vec![name.to_vec()];
        if kind == FileKind::Mf {
            let mut n = name.to_vec();
            n.extend_from_slice(b".mf");
            tries.insert(0, n);
        }
        tries
            .into_iter()
            .find_map(|n| self.0.get(&n).map(|d| (d.clone(), n.clone())))
    }
}

fn trap_config(input: &str) -> Config {
    Config {
        ini: true,
        dump_name: "inimf".into(),
        terminal_input: fixture(input),
        main_memory: 3001,
        buf_size: 500,
        error_line: 64,
        half_error_line: 32,
        max_print_line: 72,
        screen_width: 100,
        screen_depth: 200,
        gf_buf_size: 8,
        trap_screen: true,
        date: (22 * 60 + 13, 14, 11, 2023),
        version_string: " (TeX Live 2026/Arch Linux)".into(),
        ..Config::default()
    }
}

fn output<'a>(mf: &'a Mf, name: &str) -> &'a [u8] {
    &mf.outputs
        .iter()
        .find(|(n, _)| n == name.as_bytes())
        .unwrap_or_else(|| panic!("no output {name}"))
        .1
}

fn same(what: &str, ours: &[u8], theirs: &[u8]) {
    if ours != theirs {
        let o = String::from_utf8_lossy(ours);
        let t = String::from_utf8_lossy(theirs);
        let first = o
            .lines()
            .zip(t.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b);
        assert!(
            first.is_none(),
            "{what} differs at line {:?}",
            first.map(|(i, l)| (i + 1, l))
        );
        panic!(
            "{what} differs in length: {} vs {} bytes",
            ours.len(),
            theirs.len()
        );
    }
}

#[test]
fn trap() {
    let mut files = HashMap::new();
    files.insert(b"trap.mf".to_vec(), fixture("trap.mf"));
    // pass 1: mftrap1.in, INIMF dumps trap.base
    let mut mf = Mf::new(trap_config("mftrap1.in"), Box::new(TrapIo(files.clone())));
    mf.run();
    same(
        "mftrapin.log",
        output(&mf, "trap.log"),
        &fixture("mftrapin.log"),
    );
    same("mftrapin.fot", &mf.term_out, &fixture("mftrapin.fot"));
    let base = output(&mf, "trap.base").to_vec();
    files.insert(b"trap.base".to_vec(), base);
    // pass 2: mftrap2.in, from trap.base
    let mut mf = Mf::new(trap_config("mftrap2.in"), Box::new(TrapIo(files)));
    mf.run();
    same(
        "mftrap.log",
        output(&mf, "trap.log"),
        &fixture("mftrap.log"),
    );
    same("mftrap.fot", &mf.term_out, &fixture("mftrap.fot"));
    assert!(
        output(&mf, "trap.tfm") == fixture("trap.tfm"),
        "trap.tfm differs"
    );
    assert!(
        output(&mf, "trap.72270gf") == fixture("trap.72270gf"),
        "trap.72270gf differs"
    );
}
