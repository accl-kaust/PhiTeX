//! Repros from the `TikZ` manual run (agent tikzman), `#[ignore]`d with
//! the defect each shows until its fix lands.

use std::path::PathBuf;
use std::process::Command;

/// A diagnostic whose source line has bytes >= 0x80 (a koi8-r `\input`
/// file, shown as Latin-1 chars) must render: the renderer is given byte
/// offsets into the line's UTF-8 text, so it panicked "end byte index 7
/// is not a char boundary; it is inside 'Ò'" (exit 101).
#[test]
#[ignore = "the error renderer (annotate-snippets) panics on a source line with non-ASCII chars: byte offsets of the Latin-1 bytes used as UTF-8 string offsets"]
fn diagnostic_on_latin1_line_does_not_panic() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tikzman-latin1-diag");
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(
        dir.join("k7in.tex"),
        b"\\catcode `\\\xc1=12 \\catcode `\\\xc2=12 \\catcode `\\\xd2=12\n\
          \\lccode `\\\xc1=224 \\lccode `\\\xc2=225 \\lccode `\\\xd2=240\n\
          \\patterns{\n.\xc1\xc21\xd2\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("k7.tex"),
        b"\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n\\input k7in\n\\message{[done]}\n\\end\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_phitex"))
        .args([
            "build",
            "--ini",
            "--etex",
            "--output-format=pdf",
            "-v",
            "--exact-pdf",
            "-j",
            "4",
            "k7.tex",
        ])
        .current_dir(&dir)
        .env("PHITEX_CACHE_DIR", dir.join("cache"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(101), "panicked:\n{err}");
    assert!(!err.contains("panic"), "panicked:\n{err}");
}
