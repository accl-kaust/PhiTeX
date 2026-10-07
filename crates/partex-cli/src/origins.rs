//! `PARTEX_ORIGINS=1`: glyph origins (DESIGN 4.4, `partex_core::srcmap`)
//! recorded, and written after each build or rebuild to
//! `<job>.origins.jsonl` next to the PDF: line 1 `{"files":[...]}`, the
//! files by id, as the job asked for them; then a line per page,
//! `{"page":N,"glyphs":[[file,start,end,synth],...]}`, `N` from 1, the
//! glyphs in the order the page's content stream shows them, `synth` 1
//! for a synthesized glyph, `file` 4294967295 (`u32::MAX`) for a glyph
//! with no source.

use std::io::Write;

use partex_core::Tex;
use partex_core::track::Tracker;

use crate::native::NativeHost;

/// Whether origins are asked for.
pub fn wanted() -> bool {
    std::env::var("PARTEX_ORIGINS").is_ok_and(|v| v == "1")
}

/// `-synctex=N`, as web2c reads it (`strtol(optarg, NULL, 0)`), if given.
static SYNCTEX: std::sync::Mutex<Option<i32>> = std::sync::Mutex::new(None);

/// The command line's `-synctex=N`.
pub fn set_synctex(v: &str) {
    if let Ok(mut s) = SYNCTEX.lock() {
        *s = Some(strtol(v));
    }
}

/// C's `strtol(s, NULL, 0)` into an `int`: an optional sign, then `0x`
/// for hexadecimal or `0` for octal; what does not parse stops it (none:
/// 0).
fn strtol(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let (radix, digits) = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16, h)
    } else if s.starts_with('0') {
        (8, s)
    } else {
        (10, s)
    };
    let mut v: i64 = 0;
    for c in digits.chars() {
        let Some(d) = c.to_digit(radix) else { break };
        v = (v * i64::from(radix) + i64::from(d)).min(i64::from(u32::MAX) + 1);
    }
    let v = if neg { -v } else { v };
    // (`long` to `int`: the low 32 bits)
    #[allow(clippy::cast_possible_truncation, reason = "C's conversion")]
    let v = v as i32;
    v
}

/// Turn origins on for `tex` if they are asked for, and `SyncTeX` if the
/// command line asks for it.
pub fn setup<T: Tracker>(tex: &mut Tex<NativeHost, T>) {
    if wanted() {
        tex.set_origins(true);
    }
    setup_synctex(tex);
}

/// No `-synctex` (a command line parsed anew).
pub fn clear_synctex() {
    if let Ok(mut s) = SYNCTEX.lock() {
        *s = None;
    }
}

/// The command line's `-synctex=N`, if given.
pub fn synctex_option() -> Option<i32> {
    SYNCTEX.lock().ok().and_then(|s| *s)
}

/// Turn `SyncTeX` on for `tex` if the command line asks for it (any host:
/// machine mode's too, whose regions record its events).
pub fn setup_synctex<H: partex_core::Host, T: Tracker>(tex: &mut Tex<H, T>) {
    if let Some(n) = synctex_option() {
        tex.set_synctex(n);
    }
}

/// A JSON string.
pub(crate) fn json_str(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    for c in s.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => {
                let mut b = [0; 4];
                out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// The origins as the file holds them.
pub fn render<T: Tracker>(tex: &mut Tex<NativeHost, T>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"{\"files\":[");
    for (i, f) in tex.origin_files().iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        json_str(&mut out, f);
    }
    out.extend_from_slice(b"]}\n");
    for (n, page) in tex.origin_pages().iter().enumerate() {
        let _ = write!(out, "{{\"page\":{},\"glyphs\":[", n + 1);
        for (i, g) in page.iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            let _ = write!(
                out,
                "[{},{},{},{}]",
                g.file,
                g.start,
                g.end,
                u8::from(g.synthesized)
            );
        }
        out.extend_from_slice(b"]}\n");
    }
    out
}

/// Write `<job>.origins.jsonl` (if origins are on), and in SSA mode the
/// build's `SyncTeX` file (if `-synctex` asked for it).
pub fn write<T: Tracker>(tex: &mut Tex<NativeHost, T>) {
    // (SSA mode: the build's `SyncTeX` file, from its steps' events)
    tex.synctex_write();
    if !tex.origins_on() {
        return;
    }
    let Some(job) = tex.job_name_bytes() else {
        return;
    };
    let mut name = job;
    name.extend_from_slice(b".origins.jsonl");
    let name = tex.host().in_output_dir(&name).unwrap_or(name);
    let text = render(tex);
    let path = crate::native::path(&name);
    if let Err(e) = std::fs::write(&path, text) {
        eprintln!("phitex: {}: {e}", path.display());
    }
}
