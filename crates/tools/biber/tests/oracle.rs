//! The oracle: every fixture in oracle/fixtures/biber, its `.bcf` and
//! `refs.bib` fed to the native biber, must give biber 2.22's `.bbl`
//! byte for byte.

use std::path::PathBuf;

struct Dir(PathBuf);

impl phitex_biber::Files for Dir {
    fn datasource(&mut self, name: &str) -> Option<(String, Vec<u8>)> {
        let p = self.0.join(name);
        std::fs::read(&p).ok().map(|b| (name.to_string(), b))
    }
}

#[test]
fn fixtures_byte_identical() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../oracle/fixtures/biber");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("fixtures")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".bcf").map(String::from))
        .collect();
    names.sort();
    let mut bad = Vec::new();
    for n in &names {
        let bcf = std::fs::read(dir.join(format!("{n}.bcf"))).expect("bcf");
        let want = std::fs::read_to_string(dir.join(format!("{n}.bbl"))).expect("bbl");
        let out = phitex_biber::run(&bcf, &mut Dir(dir.clone()));
        match &out.bbl {
            Some(got) if *got == want => println!("IDENTICAL {n}"),
            Some(got) => {
                let line = got.lines().zip(want.lines()).position(|(a, b)| a != b).unwrap_or(0);
                println!("DIFFERS   {n} (first differing line {})", line + 1);
                println!("  got:  {:?}", got.lines().nth(line));
                println!("  want: {:?}", want.lines().nth(line));
                let _ = std::fs::create_dir_all("target/biber-oracle");
                let _ = std::fs::write(format!("target/biber-oracle/{n}.bbl"), got);
                bad.push(n.clone());
            }
            None => {
                println!("FAILED    {n}: {:?} unsupported {:?}", out.errors, out.unsupported);
                bad.push(n.clone());
            }
        }
    }
    assert!(bad.is_empty(), "differing: {bad:?}");
}
