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

/// Turn origins on for `tex` if they are asked for.
pub fn setup<T: Tracker>(tex: &mut Tex<NativeHost, T>) {
    if wanted() {
        tex.set_origins(true);
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

/// Write `<job>.origins.jsonl` (if origins are on).
pub fn write<T: Tracker>(tex: &mut Tex<NativeHost, T>) {
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
        eprintln!("partex: {}: {e}", path.display());
    }
}
