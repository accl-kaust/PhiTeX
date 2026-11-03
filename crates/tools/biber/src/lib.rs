//! biber 2.22, run in process: reads a job's `.bcf` control file and its
//! `.bib` data sources and produces the `.bbl` (byte-identical to biber's)
//! and a `.blg` log (not a gate).
//!
//! A port of biber's behaviour from its Perl modules (Biber.pm,
//! Biber::Config, Biber::DataModel, Biber::Input::file::bibtex,
//! Biber::Internals, Biber::DataList, Biber::Output::bbl,
//! Biber::LaTeX::Recode, Biber::Date::Format, btparse); each module
//! names the one it ports. Features the port does not cover make the run
//! fail plainly with [`Outcome::unsupported`] listing them.
//!
//! `no_std` + `alloc`: files come through [`Files`]; no processes.

#![no_std]
#![allow(
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::struct_excessive_bools,
    clippy::struct_field_names,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::module_name_repetitions,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::fn_params_excessive_bools,
    clippy::needless_pass_by_value,
    clippy::float_cmp,
    // The port keeps biber's Perl structure (its branch-per-case
    // conditionals, its names, its string building), so it reads
    // against the Perl modules:
    clippy::match_same_arms,
    clippy::if_same_then_else,
    clippy::collapsible_if,
    clippy::collapsible_else_if,
    clippy::many_single_char_names,
    clippy::doc_markdown,
    clippy::format_push_string,
    clippy::unnecessary_wraps,
    clippy::items_after_statements,
    clippy::unreadable_literal,
    clippy::single_match_else,
    clippy::bool_to_int_with_if
)]

extern crate alloc;

mod bbl;
mod bcf;
mod bib;
mod biber;
mod btparse;
mod collate;
mod config;
mod date;
mod internals;
mod lists;
mod locales;
mod model;
mod recode;
mod regex;
mod section;
mod unicode;
mod unidata;
mod xml;

use alloc::string::String;
use alloc::vec::Vec;

/// Where the run's data sources come from.
pub trait Files {
    /// A data source named in the `.bcf` (`\addbibresource{refs.bib}`
    /// gives `refs.bib`), found the way biber finds it (as given, then
    /// through kpathsea's `bib` format): the name it was found under and
    /// its bytes.
    fn datasource(&mut self, name: &str) -> Option<(String, Vec<u8>)>;
}

/// What a run produced.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// The `.bbl` text (absent when the run failed).
    pub bbl: Option<String>,
    /// The `.blg` log.
    pub blg: String,
    /// Warnings biber would print.
    pub warnings: Vec<String>,
    /// Errors (a fatal one ends the run without a `.bbl`).
    pub errors: Vec<String>,
    /// Features of the input this port does not support; the run fails
    /// when there are any.
    pub unsupported: Vec<String>,
    /// The `.bbl` written in Latin-1 (`output_encoding`), not UTF-8.
    latin1: bool,
}

impl Outcome {
    /// The `.bbl` as the file's bytes, in its output encoding (its
    /// characters are all in it, [`Outcome::bbl`] recoded).
    pub fn bbl_bytes(&self) -> Option<Vec<u8>> {
        let t = self.bbl.as_ref()?;
        Some(if self.latin1 {
            t.chars()
                .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
                .collect()
        } else {
            t.clone().into_bytes()
        })
    }

    /// biber's exit status: 2 on a fatal error, else 0.
    pub fn status(&self) -> i32 {
        if self.bbl.is_none() { 2 } else { 0 }
    }
}

/// The file biber makes by default from its control file, as latexmk
/// runs it when a job reads `X.bbl` and writes `X.bcf`: the input for
/// `output` (`X.bcf` for `X.bbl`), if `output` is that default output's
/// name.
#[must_use]
pub fn default_input_for(output: &str) -> Option<String> {
    let n = output.len();
    (n > 4 && output.as_bytes()[n - 4..].eq_ignore_ascii_case(b".bbl"))
        .then(|| alloc::format!("{}.bcf", &output[..n - 4]))
}

/// A `.bcf` as biber reads it, the same `.bbl` from the same data sources,
/// whatever citations of keys cited already it lists: of a section's
/// `citekey` elements, a later one of a key (with its `nocite` the same) is
/// passed over (`Biber::parse_ctrlfile`: the first gives the key's place,
/// [`bcf`]'s `seenkeys`), and `order`, which only the sort field `citeorder`
/// reads, becomes its rank where no sorting template a data list uses has
/// that field (where one does, its digits reach the `.bbl`: `sortinit` is the
/// sort string's first character). So adding a `\cite` of a key cited before,
/// which renumbers every later element, makes no new input. `None`: left as
/// it is (`*`, citeorder sorted, a data list with no template named, an order
/// that is no positive number, a citekey line of another form).
#[must_use]
pub fn canonical_bcf(bcf: &str) -> Option<String> {
    if citeorder_read(bcf)? {
        return None;
    }
    // (each line kept, a kept citekey's the span of its order and the number)
    let mut lines: Vec<(&str, Option<(usize, usize, f64)>)> = Vec::new();
    let mut seen: Vec<(&str, bool)> = Vec::new();
    let mut out = String::with_capacity(bcf.len());
    let rank = |lines: &[(&str, Option<(usize, usize, f64)>)], out: &mut String| {
        let mut ns: Vec<f64> = lines.iter().filter_map(|x| x.1.map(|o| o.2)).collect();
        ns.sort_by(f64::total_cmp);
        ns.dedup();
        for (l, o) in lines {
            match o {
                Some((a, b, n)) => {
                    let r = ns.partition_point(|x| x < n) + 1;
                    out.push_str(&l[..*a]);
                    out.push_str(&alloc::format!("{r}"));
                    out.push_str(&l[*b..]);
                }
                None => out.push_str(l),
            }
        }
    };
    for line in bcf.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("<bcf:citekey ") && !t.contains(" type=") {
            let (head, rest) = t.split_once('>')?;
            let inner = rest.trim_end().strip_suffix("</bcf:citekey>")?;
            if inner == "*" {
                return None;
            }
            let nocite = head.contains("nocite=");
            if seen.contains(&(inner, nocite)) {
                continue;
            }
            seen.push((inner, nocite));
            let a = line.find(" order=\"")? + 8;
            let b = a + line[a..].find('"')?;
            let n: f64 = line[a..b].parse().ok()?;
            if !(n > 0.0 && n.is_finite()) {
                return None;
            }
            lines.push((line, Some((a, b, n))));
            continue;
        }
        // (a section's end: its orders ranked among themselves)
        if t.starts_with("</bcf:section") {
            rank(&lines, &mut out);
            lines.clear();
            seen.clear();
            out.push_str(line);
            continue;
        }
        lines.push((line, None));
    }
    rank(&lines, &mut out);
    Some(out)
}

/// Whether a sorting template some data list of the `.bcf` names sorts by
/// `citeorder`. `None`: a data list names none.
fn citeorder_read(bcf: &str) -> Option<bool> {
    const USED: &str = "sortingtemplatename=\"";
    const NAME: &str = "<bcf:sortingtemplate name=\"";
    let quoted = |s: &str| s.split('"').next().map(alloc::string::String::from);
    let mut used = Vec::new();
    for l in bcf.lines() {
        if let Some((_, r)) = l.split_once(USED) {
            used.push(quoted(r)?);
        }
    }
    if used.len() != bcf.matches("<bcf:datalist ").count() {
        return None;
    }
    let mut current = None;
    for l in bcf.lines() {
        let t = l.trim_start();
        if let Some(r) = t.strip_prefix(NAME) {
            current = quoted(r);
        } else if t.contains(">citeorder<") && current.as_ref().is_some_and(|c| used.contains(c)) {
            return Some(true);
        }
    }
    (!used.is_empty()).then_some(false)
}

/// Run biber on a `.bcf` (its bytes) with data sources from `files`.
pub fn run(bcf: &[u8], files: &mut dyn Files) -> Outcome {
    let mut b = biber::Biber::new(files);
    let text = String::from_utf8_lossy(bcf).into_owned();
    // (no data sources: biber's "Be friendly to latexmk" exit, `EXIT_OK`
    // after `parse_ctrlfile`, its .bbl opened and truncated before it and
    // left empty)
    let r = b
        .parse_ctrlfile(&text)
        .and_then(|()| if b.no_data { Ok(()) } else { b.prepare() });
    let bbl = match r {
        Ok(()) if b.no_data => Some(String::new()),
        Ok(()) => Some(b.bbl_text()),
        Err(f) => {
            if !b.errors.contains(&f.0) {
                b.errors.push(f.0);
            }
            None
        }
    };
    let mut blg = String::new();
    blg.push_str("INFO - This is the native biber (biber 2.22 port)\n");
    for l in &b.log {
        blg.push_str(&alloc::format!("INFO - {l}\n"));
    }
    for w in b.warnings.iter().chain(b.bt_warnings.iter()) {
        blg.push_str(&alloc::format!("WARN - {w}\n"));
    }
    for e in &b.errors {
        blg.push_str(&alloc::format!("ERROR - {e}\n"));
    }
    let mut warnings = b.warnings.clone();
    warnings.extend(b.bt_warnings.iter().cloned());
    let latin1 =
        recode::Encoding::of(&b.cfg.opt("output_encoding")) == Some(recode::Encoding::Latin1);
    Outcome {
        bbl,
        blg,
        warnings,
        errors: b.errors.clone(),
        unsupported: b.unsupported.clone(),
        latin1,
    }
}
