use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PATH");
    let suffix = Command::new("pdftex")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            let line = s.lines().next()?;
            let (_, suffix) = line.split_once("1.40.29")?;
            Some(suffix.to_owned())
        })
        .unwrap_or_else(|| " (TeX Live 2026)".to_owned());
    println!("cargo:rustc-env=PARTEX_ORACLE_VERSION_SUFFIX={suffix}");
}
