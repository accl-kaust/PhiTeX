//! The files xdvipdfmx reads: kpathsea's lookups and the file system, as
//! the host gives them.

use alloc::sync::Arc;
use alloc::vec::Vec;

/// kpathsea's file formats dvipdfm-x looks files up in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// `kpse_fontmap_format` (`.map`; also AGL `.txt` files).
    Fontmap,
    /// `kpse_type1_format` (`.pfb`, `.pfa`).
    Type1,
    /// `kpse_truetype_format` (`.ttf`, `.ttc`, `.dfont`).
    TrueType,
    /// `kpse_opentype_format` (`.otf`).
    OpenType,
    /// `kpse_cmap_format`.
    Cmap,
    /// `kpse_sfd_format`.
    Sfd,
    /// `kpse_enc_format`.
    Enc,
    /// `kpse_tfm_format`.
    Tfm,
    /// `kpse_ofm_format`.
    Ofm,
    /// `kpse_vf_format`.
    Vf,
    /// `kpse_ovf_format`.
    Ovf,
    /// `kpse_pict_format` (graphics).
    Pict,
    /// `kpse_tex_format`.
    Tex,
    /// `kpse_program_text_format`.
    ProgramText,
    /// `kpse_program_binary_format`.
    ProgramBinary,
}

/// What the host gives xdvipdfmx.
pub trait Files {
    /// `kpse_find_file(name, format, 0)` as the program `progname` (dvipdfm-x
    /// searches as `dvipdfmx`, and "fools" kpathsea with other names for
    /// some formats): the path found.
    fn find(&mut self, name: &[u8], format: Format, progname: &[u8]) -> Option<Vec<u8>>;

    /// A file's contents by path (a path `find` returned, or an absolute
    /// path from the XDV).
    fn read(&mut self, path: &[u8]) -> Option<Arc<[u8]>>;

    /// `dpx_file_apply_filter` for `ps_include_page`: the distiller
    /// (`dvipdfmx.cfg`'s `D` line, `template`) run on the file at `input`
    /// (a path `find` returned), its output file's bytes. The host picks
    /// the output file and runs [`filter_command`]'s command, as C's
    /// `exec_spawn` would; `None`: it failed, or the host runs no
    /// commands (the default).
    fn apply_filter(&mut self, template: &[u8], input: &[u8], version: i32) -> Option<Arc<[u8]>> {
        let _ = (template, input, version);
        None
    }
}

/// `dpx_file_apply_filter`'s command: `template` with `%o` the output
/// file, `%i` the input, `%b` the input without its suffix, `%v` the PDF
/// version (`1.5` for `version` 15), `%%` a `%`.
#[must_use]
pub fn filter_command(template: &[u8], input: &[u8], output: &[u8], version: i32) -> Vec<u8> {
    let mut cmd = Vec::with_capacity(template.len() + input.len() + output.len() + 3);
    let mut p = template.iter();
    while let Some(&c) = p.next() {
        if c != b'%' {
            cmd.push(c);
            continue;
        }
        match p.next() {
            Some(b'o') => cmd.extend_from_slice(output),
            Some(b'i') => cmd.extend_from_slice(input),
            Some(b'b') => match input.iter().rposition(|&b| b == b'.') {
                Some(k) => cmd.extend_from_slice(&input[..k]),
                None => cmd.extend_from_slice(input),
            },
            Some(b'v') => {
                let v = alloc::format!("{}.{}", version / 10, version % 10);
                cmd.extend_from_slice(v.as_bytes());
            }
            Some(b'%') => cmd.push(b'%'),
            // (C: any other character after `%` is dropped with it)
            _ => {}
        }
    }
    cmd
}

#[cfg(test)]
mod filter_tests {
    use super::filter_command;

    #[test]
    fn filter_template() {
        let c = filter_command(b"rungs -dCompatibilityLevel=%v -sOutputFile='%o' '%i' %b 100%% %q", b"/d/a.eps", b"/t/o.pdf", 15);
        assert_eq!(c, b"rungs -dCompatibilityLevel=1.5 -sOutputFile='/t/o.pdf' '/d/a.eps' /d/a 100% ".to_vec());
    }
}
