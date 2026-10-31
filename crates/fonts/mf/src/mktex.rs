//! Fonts made as TeX Live makes them on demand: the base as `fmtutil`
//! builds it (`mf-nowin -ini -jobname=mf -translate-file=cp227.tcx
//! mf.ini`), the TFM as `mktextfm` runs METAFONT, the PK as `mktexpk`
//! runs it (the magnification as kpathsea's `tex-make.c` writes
//! `MAKETEX_MAG`) and `gftopk` packs its GF file.

use crate::{Config, FileKind, Io, Mf};

/// A run of METAFONT: its history, terminal output and output files.
pub struct Run {
    pub history: i32,
    pub term: Vec<u8>,
    pub outputs: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Run {
    /// The output file named `name`.
    #[must_use]
    pub fn output(&self, name: &str) -> Option<&[u8]> {
        self.outputs
            .iter()
            .find(|(n, _)| n == name.as_bytes())
            .map(|(_, d)| d.as_slice())
    }
    /// The output file whose name ends in `suffix`.
    #[must_use]
    pub fn output_ending(&self, suffix: &str) -> Option<(&[u8], &[u8])> {
        self.outputs
            .iter()
            .find(|(n, _)| n.ends_with(suffix.as_bytes()))
            .map(|(n, d)| (n.as_slice(), d.as_slice()))
    }
    fn failure(&self, what: &str) -> String {
        format!(
            "{what}: METAFONT failed:\n{}",
            String::from_utf8_lossy(&self.term)
        )
    }
}

/// The tables of `read_tcx_file` (lib/texmfmp.c) applied to the defaults
/// of `mf.web` §21-§23: `(xord, xchr, xprn)`.
#[must_use]
pub fn tcx_tables(tcx: &[u8], eight_bit: bool) -> (Box<[u8; 256]>, Box<[u8; 256]>, Box<[u8; 256]>) {
    let mut xchr = Box::new([0u8; 256]);
    let mut xord = Box::new([0o177u8; 256]);
    let mut xprn = Box::new([0u8; 256]);
    for i in 0..256 {
        xchr[i] = i as u8;
    }
    for i in 0o200..=0o377usize {
        xord[xchr[i] as usize] = i as u8;
    }
    for i in 0..=0o176usize {
        xord[xchr[i] as usize] = i as u8;
    }
    for i in 0..256usize {
        xprn[i] = u8::from(eight_bit || (32..=126).contains(&i));
    }
    for line in tcx.split(|&b| b == b'\n') {
        let line = match line.iter().position(|&b| b == b'%') {
            Some(k) => &line[..k],
            None => line,
        };
        let (first, rest) = tcx_num(line, 255);
        let Some(first) = first else { continue };
        let (second, rest) = tcx_num(rest, 255);
        let (second, printable) = match second {
            Some(second) => {
                xord[first as usize] = second as u8;
                xchr[second as usize] = first as u8;
                let mut printable = tcx_num(rest, 1).0.unwrap_or(1);
                if (32..=126).contains(&second) {
                    printable = 1;
                }
                (second, printable)
            }
            None => (first, 1),
        };
        xprn[second as usize] = printable as u8;
    }
    (xord, xchr, xprn)
}

/// `tcx_get_num`: C's `strtol(s, &end, 0)`, `None` where there is no
/// number or it is outside `0..=upb`, and the rest of the line.
fn tcx_num(s: &[u8], upb: i64) -> (Option<i64>, &[u8]) {
    let mut i = 0;
    while i < s.len() && (s[i] == b' ' || (9..=13).contains(&s[i])) {
        i += 1;
    }
    let mut neg = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        neg = s[i] == b'-';
        i += 1;
    }
    let (radix, mut j) = if s[i..].starts_with(b"0x") || s[i..].starts_with(b"0X") {
        if s.get(i + 2).is_some_and(u8::is_ascii_hexdigit) {
            (16, i + 2)
        } else {
            (8, i + 1)
        }
    } else if s.get(i) == Some(&b'0') {
        (8, i)
    } else {
        (10, i)
    };
    let start = j;
    let mut n: i64 = 0;
    while let Some(d) = s.get(j).and_then(|&c| (c as char).to_digit(radix)) {
        n = n
            .saturating_mul(i64::from(radix))
            .saturating_add(i64::from(d));
        j += 1;
    }
    if j == start && !(radix == 8 && start > i) {
        return (None, s);
    }
    let n = if neg { -n } else { n };
    if n < 0 || n > upb {
        (None, &s[j..])
    } else {
        (Some(n), &s[j..])
    }
}

/// The files METAFONT reads, and the base it loads.
struct WithBase {
    io: Box<dyn Io>,
    base: Option<(Vec<u8>, String)>,
}

impl Io for WithBase {
    fn open_in(&mut self, name: &[u8], kind: FileKind) -> Option<(Vec<u8>, Vec<u8>)> {
        if kind == FileKind::Base {
            let (b, n) = self.base.as_ref()?;
            return (name == format!("{n}.base").as_bytes()).then(|| (b.clone(), name.to_vec()));
        }
        self.io.open_in(name, kind)
    }
}

/// Settings for the runs of METAFONT a TeX Live installation makes:
/// texmf.cnf's values for `mf` and `inimf`.
#[derive(Clone, Debug)]
pub struct Settings {
    /// `MAKETEX_MODE` (mktex.cnf: `ljfour`).
    pub mode: String,
    /// The date and time (`date_and_time`): minutes after midnight, day,
    /// month, year.
    pub date: (i32, i32, i32, i32),
    /// web2c's version string.
    pub version_string: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mode: "ljfour".into(),
            date: (22 * 60 + 13, 14, 11, 2023),
            version_string: " (TeX Live 2026)".into(),
        }
    }
}

fn run(cfg: Config, io: Box<dyn Io>, base: Option<(&[u8], &str)>) -> Run {
    let wb = WithBase {
        io,
        base: base.map(|(b, n)| (b.to_vec(), n.to_string())),
    };
    let mut mf = Mf::new(cfg, Box::new(wb));
    let history = mf.run();
    Run {
        history,
        term: std::mem::take(&mut mf.term_out),
        outputs: std::mem::take(&mut mf.outputs),
    }
}

/// The base `mf.base` as fmtutil builds it: INIMF on `mf.ini` with the
/// TCX file `tcx` (its name and bytes; TeX Live's `cp227.tcx`).
///
/// # Errors
/// METAFONT's terminal output where no base was dumped.
pub fn build_base(
    io: Box<dyn Io>,
    tcx: Option<(&str, &[u8])>,
    s: &Settings,
) -> Result<Vec<u8>, String> {
    let translate = tcx.map(|(n, d)| {
        let (o, c, p) = tcx_tables(d, false);
        (n.to_string(), o, c, p)
    });
    let cfg = Config {
        ini: true,
        dump_name: "mf".into(),
        job_name: Some("mf".into()),
        command_line: vec![b"mf.ini".to_vec()],
        translate,
        date: s.date,
        version_string: s.version_string.clone(),
        ..Config::default()
    };
    let r = run(cfg, io, None);
    r.output("mf.base")
        .map(<[u8]>::to_vec)
        .ok_or_else(|| r.failure("mf.base"))
}

/// `kpse_magstep_fix(dpi, bdpi)` (kpathsea `magstep.c`): the resolution
/// and kpathsea's `m_ret` (a comparison, so 0 but where the resolution is
/// the magstep's number).
#[must_use]
pub fn magstep_fix(dpi: i32, bdpi: i32) -> (i32, i32) {
    let sign = if dpi < bdpi { -1 } else { 1 };
    let mut real = 0;
    let mut m = 0;
    let mut mdpi = -1;
    while real == 0 && m < 40 {
        mdpi = magstep(m * sign, bdpi);
        if (mdpi - dpi).abs() <= 1 {
            real = mdpi;
        } else if (mdpi - dpi) * sign > 0 {
            real = dpi;
        }
        m += 1;
    }
    // (`*m_ret = real_dpi == (unsigned) (mdpi ? (m - 1) * sign : 0)`: a
    // comparison, 1 only where the resolution is the magstep's number
    // itself, so MAKETEX_MAG is `DPI/BDPI+DPI%BDPI/BDPI` at magsteps too,
    // as TeX Live's mktexpk is run: `--mag 1+264/600 --dpi 864`, mc job
    // 77344)
    let mret = i32::from(real == if mdpi == 0 { 0 } else { (m - 1) * sign });
    (if real == 0 { dpi } else { real }, mret)
}

#[allow(clippy::cast_possible_truncation)]
fn magstep(n: i32, bdpi: i32) -> i32 {
    let neg = n < 0;
    let mut n = n.abs();
    let mut t = if n & 1 == 1 {
        n &= !1;
        1.095_445_115
    } else {
        1.0
    };
    while n > 8 {
        n -= 8;
        t *= 2.0736;
    }
    while n > 0 {
        n -= 2;
        t *= 1.2;
    }
    let b = f64::from(bdpi);
    (0.5 + if neg { b / t } else { b * t }) as i32
}

/// `MAKETEX_MAG` (kpathsea `tex-make.c` `set_maketex_mag`) for a font at
/// `dpi` of base resolution `bdpi`, as `mktexpk` hands it to METAFONT.
#[must_use]
pub fn maketex_mag(dpi: i32, bdpi: i32) -> String {
    let (_, m) = magstep_fix(dpi, bdpi);
    if m == 0 {
        let (a, b) = (dpi / bdpi, dpi % bdpi);
        if bdpi <= 4000 {
            format!("{a}+{b}/{bdpi}")
        } else {
            let (f, r) = (bdpi / 4000, bdpi % 4000);
            if f > 1 {
                if r > 0 {
                    format!("{a}+{b}/({f}*4000+{r})")
                } else {
                    format!("{a}+{b}/({f}*4000)")
                }
            } else {
                format!("{a}+{b}/(4000+{r})")
            }
        }
    } else {
        let (sign, m) = if m < 0 { ("-", -m) } else { ("", m) };
        format!("magstep({sign}{}.{})", m / 2, (m & 1) * 5)
    }
}

/// mktexpk's guess of a mode from base resolution `bdpi`.
#[must_use]
pub fn guess_mode(bdpi: i32) -> Option<&'static str> {
    Some(match bdpi {
        85 => "sun",
        100 => "nextscrn",
        180 => "toshiba",
        300 => "cx",
        360 => "epstylus",
        400 => "nexthi",
        600 => "ljfour",
        720 => "epscszz",
        1200 => "ultre",
        1270 => "linoone",
        8000 => "dpdfezzz",
        _ => return None,
    })
}

/// The mode mktexpk runs METAFONT in: `mode` (`--mfmode`, else mktex.opt's
/// `ljfour`) where METAFONT's `pixels_per_inch` for it is `bdpi`, else
/// its guess from `bdpi` ([`guess_mode`]).
///
/// # Errors
/// mktexpk's message where it can guess none.
pub fn mktexpk_mode(
    io: Box<dyn Io>,
    base: &[u8],
    mode: Option<&str>,
    bdpi: i32,
    s: &Settings,
) -> Result<String, String> {
    let mode = mode
        .filter(|m| !m.is_empty() && *m != "/")
        .unwrap_or("ljfour");
    let line =
        format!("\\mode:={mode};mode_setup;message\"BDPI= \"&decimal round pixels_per_inch;end.");
    let r = font_run(io, base, &line, s);
    // (`awk '/DPI=/ {print $2}'`)
    let mf_bdpi = r
        .term
        .split(|&b| b == b'\n')
        .filter(|l| l.windows(4).any(|w| w == b"DPI="))
        .filter_map(|l| {
            l.split(|b| b.is_ascii_whitespace())
                .filter(|w| !w.is_empty())
                .nth(1)
                .map(<[u8]>::to_vec)
        })
        .next_back();
    if mf_bdpi.as_deref() == Some(bdpi.to_string().as_bytes()) && mode != "default" {
        return Ok(mode.to_string());
    }
    guess_mode(bdpi)
        .map(str::to_string)
        .ok_or_else(|| format!("mktexpk: Can't guess mode for {bdpi} dpi devices."))
}

/// A font METAFONT made: the PK at the resolution asked for, and the TFM.
pub struct Made {
    /// `NAME.DPIpk`.
    pub pk_name: String,
    pub pk: Vec<u8>,
    pub gf: Vec<u8>,
    pub tfm: Option<Vec<u8>>,
}

/// `mktexpk --dpi DPI --bdpi BDPI --mag MAKETEX_MAG --mfmode MODE NAME`:
/// METAFONT on `NAME` from `base`, the GF packed by gftopk.
///
/// # Errors
/// What went wrong, METAFONT's terminal output where it made no GF file.
pub fn mktexpk(
    io: Box<dyn Io>,
    base: &[u8],
    name: &str,
    dpi: i32,
    bdpi: i32,
    s: &Settings,
) -> Result<Made, String> {
    let mag = maketex_mag(dpi, bdpi);
    let line = format!("\\mode:={}; mag:={mag}; nonstopmode; input {name}", s.mode);
    let r = font_run(io, base, &line, s);
    let gf_name = format!("{name}.{dpi}gf");
    let gf = r
        .output(&gf_name)
        .ok_or_else(|| r.failure(&gf_name))?
        .to_vec();
    let pk = phitex_gftopk::gf_to_pk(&gf)?;
    Ok(Made {
        pk_name: format!("{name}.{dpi}pk"),
        pk,
        gf,
        tfm: r.output(&format!("{name}.tfm")).map(<[u8]>::to_vec),
    })
}

/// `mktextfm NAME`: METAFONT on `NAME` at `mag:=1` in mode `MODE`.
///
/// # Errors
/// METAFONT's terminal output where it made no TFM file.
pub fn mktextfm(io: Box<dyn Io>, base: &[u8], name: &str, s: &Settings) -> Result<Vec<u8>, String> {
    let line = format!("\\mode:={}; mag:=1; ; nonstopmode; input {name}", s.mode);
    let r = font_run(io, base, &line, s);
    let t = format!("{name}.tfm");
    r.output(&t)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| r.failure(&t))
}

/// METAFONT as `mf-nowin -progname=mf LINE` with `mf.base`.
#[must_use]
pub fn font_run(io: Box<dyn Io>, base: &[u8], line: &str, s: &Settings) -> Run {
    let cfg = Config {
        ini: false,
        dump_name: "mf".into(),
        command_line: vec![line.as_bytes().to_vec()],
        date: s.date,
        version_string: s.version_string.clone(),
        ..Config::default()
    };
    run(cfg, io, Some((base, "mf")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maketex_mags() {
        assert_eq!(maketex_mag(600, 600), "1+0/600");
        // (kpathsea's, at magsteps too: TeX Live's mktexpk runs, mc job 77344)
        assert_eq!(maketex_mag(864, 600), "1+264/600");
        assert_eq!(maketex_mag(264, 600), "0+264/600");
        assert_eq!(maketex_mag(720, 600), "1+120/600");
        assert_eq!(maketex_mag(1000, 600), "1+400/600");
        assert_eq!(maketex_mag(60, 600), "0+60/600");
    }
}
