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
                let line = got
                    .lines()
                    .zip(want.lines())
                    .position(|(a, b)| a != b)
                    .unwrap_or(0);
                println!("DIFFERS   {n} (first differing line {})", line + 1);
                println!("  got:  {:?}", got.lines().nth(line));
                println!("  want: {:?}", want.lines().nth(line));
                let _ = std::fs::create_dir_all("target/biber-oracle");
                let _ = std::fs::write(format!("target/biber-oracle/{n}.bbl"), got);
                println!("{}", out.blg);
                bad.push(n.clone());
            }
            None => {
                println!(
                    "FAILED    {n}: {:?} unsupported {:?}",
                    out.errors, out.unsupported
                );
                bad.push(n.clone());
            }
        }
    }
    assert!(bad.is_empty(), "differing: {bad:?}");
}

/// `canonical_bcf`: every fixture's `.bcf`, and the same with its orders
/// renumbered and citations of a key cited before added, gives the same
/// `.bbl` as its canonical form.
#[test]
fn canonical_bcf_gives_the_same_bbl() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../oracle/fixtures/biber");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("fixtures")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".bcf").map(String::from))
        .collect();
    names.sort();
    let mut done = 0;
    for n in &names {
        let bcf = std::fs::read_to_string(dir.join(format!("{n}.bcf"))).expect("bcf");
        let Some(c) = phitex_biber::canonical_bcf(&bcf) else {
            continue;
        };
        let want = phitex_biber::run(bcf.as_bytes(), &mut Dir(dir.clone())).bbl;
        let got = phitex_biber::run(c.as_bytes(), &mut Dir(dir.clone())).bbl;
        assert_eq!(got, want, "{n}: the canonical input's .bbl");
        // (the first key cited again after each element: later orders shift)
        let mut key = None;
        let mut order = 0;
        let mut more = String::new();
        for l in bcf.split_inclusive('\n') {
            let t = l.trim_start();
            if t.starts_with("<bcf:citekey ") && !t.contains(" type=") && !t.contains("nocite=") {
                let (_, rest) = t.split_once('>').expect("element");
                let k = rest.trim_end().strip_suffix("</bcf:citekey>").expect("end");
                let k = key.get_or_insert_with(|| k.to_owned()).clone();
                let a = t.find(" order=\"").expect("order") + 8;
                let b = a + t[a..].find('"').expect("order");
                order += 2;
                more.push_str(&format!(
                    "    <bcf:citekey order=\"{}\" intorder=\"1\">{k}</bcf:citekey>\n",
                    order + 1_000_000
                ));
                more.push_str(&t[..a]);
                more.push_str(&format!("{order}"));
                more.push_str(&t[b..]);
            } else {
                more.push_str(l);
            }
        }
        let Some(mc) = phitex_biber::canonical_bcf(&more) else {
            continue;
        };
        done += 1;
        let got = phitex_biber::run(mc.as_bytes(), &mut Dir(dir.clone())).bbl;
        let direct = phitex_biber::run(more.as_bytes(), &mut Dir(dir.clone())).bbl;
        assert_eq!(got, direct, "{n}: with citations added");
    }
    assert!(done > 3, "fixtures tried: {done}");
}

#[test]
fn a_citation_of_a_key_cited_before_makes_no_new_input() {
    let dl = "<bcf:datalist section=\"0\" sortingtemplatename=\"nty\">\n";
    let a = "<bcf:section number=\"0\">\n    <bcf:citekey order=\"1\" intorder=\"1\">a</bcf:citekey>\n    <bcf:citekey order=\"2\" intorder=\"1\">b</bcf:citekey>\n  </bcf:section>\n";
    let b = "<bcf:section number=\"0\">\n    <bcf:citekey order=\"1\" intorder=\"1\">a</bcf:citekey>\n    <bcf:citekey order=\"2\" intorder=\"1\">a</bcf:citekey>\n    <bcf:citekey order=\"3\" intorder=\"1\">b</bcf:citekey>\n  </bcf:section>\n";
    assert_eq!(
        phitex_biber::canonical_bcf(a),
        phitex_biber::canonical_bcf(b)
    );
    let star = format!(
        "<bcf:section number=\"0\">\n    <bcf:citekey order=\"1\" intorder=\"1\">*</bcf:citekey>\n  </bcf:section>\n{dl}"
    );
    assert_eq!(phitex_biber::canonical_bcf(&star), None);
}
