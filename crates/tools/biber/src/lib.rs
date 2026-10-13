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
    clippy::float_cmp
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
}

impl Outcome {
    /// biber's exit status: 2 on a fatal error, else 0.
    pub fn status(&self) -> i32 {
        if self.bbl.is_none() { 2 } else { 0 }
    }
}

/// Run biber on a `.bcf` (its bytes) with data sources from `files`.
pub fn run(bcf: &[u8], files: &mut dyn Files) -> Outcome {
    let mut b = biber::Biber::new(files);
    let text = String::from_utf8_lossy(bcf).into_owned();
    let r = b.parse_ctrlfile(&text).and_then(|()| b.prepare());
    let bbl = match r {
        Ok(()) => Some(b.bbl_text()),
        Err(_) => None,
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
    Outcome {
        bbl,
        blg,
        warnings,
        errors: b.errors.clone(),
        unsupported: b.unsupported.clone(),
    }
}
