//! Capacity parameters. web2c reads them from `texmf.cnf` at startup
//! (§1332, `main_body` as changed by tex.ch). They are observable (overflow messages,
//! end-of-run statistics), so the host must supply the values the reference
//! binary would read; the core then clamps them exactly as `const_chk` does.

/// Values web2c's `tex` takes from `texmf.cnf` before clamping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Params {
    /// Which engine to be. The data layout is always pdfTeX's (a superset
    /// of TeX's); the flavor selects the primitives, the few layout sizes
    /// that are observable in memory statistics, and the behaviors.
    pub flavor: Flavor,
    pub mem_bot: i32,
    pub main_memory: i32,
    pub extra_mem_top: i32,
    pub extra_mem_bot: i32,
    pub pool_size: i32,
    pub string_vacancies: i32,
    pub pool_free: i32,
    pub max_strings: i32,
    pub strings_free: i32,
    pub font_mem_size: i32,
    pub font_max: i32,
    pub trie_size: i32,
    pub hyph_size: i32,
    pub buf_size: i32,
    pub nest_size: i32,
    pub max_in_open: i32,
    pub param_size: i32,
    pub save_size: i32,
    pub stack_size: i32,
    pub dvi_buf_size: i32,
    pub error_line: i32,
    pub half_error_line: i32,
    pub max_print_line: i32,
    pub hash_extra: i32,
    pub expand_depth: i32,
    /// The pdfTeX bugs to reproduce (all by default; `PARTEX_PDFTEX_BUGS`).
    pub pdftex_bugs: partex_engine::bugs::PdftexBugs,
    /// `-8bit`: every character is printable (§24, `xprn`).
    pub eight_bit: bool,
    /// `-translate-file`: a TCX file's translation (it overrides the
    /// format's tables).
    pub translation: Option<crate::charset::Translation>,
    /// `-mltex`: enable `MLTeX` character substitution (INITEX only).
    pub mltex: bool,
    /// `-enc`: enable `encTeX` (INITEX only). Not supported beyond its
    /// primitives yet.
    pub enctex: bool,
    /// `-file-line-error`: `file:line: error` style messages (§73).
    pub file_line_error_style: bool,
    /// web2c's `parse_first_line_p` (the log says so).
    pub parse_first_line: bool,
    /// `-halt-on-error`: stop at the first error (§82).
    pub halt_on_error: bool,
    /// `-interaction=...` (§73–§74): `None` is `unspecified_mode`.
    pub interaction: Option<i32>,
    /// Build a structured [`crate::diag::Diagnostic`] for every error.
    /// Never changes TeX's own output.
    pub diagnostics: bool,
    /// web2c's `shellenabledp`: `\write18` is enabled. TeX Live's
    /// `texmf.cnf` disables it for `tex` (`\ifeof18` is then true).
    pub shell_escape: bool,
    /// web2c's `restrictedshell` (only commands from `shell_escape_commands`).
    pub restricted_shell: bool,
    /// `-etex` (pdfTeX's `etex_p`): enter e-TeX's extended mode in INITEX
    /// without a `*` on the first line.
    pub etex: bool,
    /// `-output-format` (pdfTeX's `fixed_pdfoutput`): 0 for DVI, 1 for PDF;
    /// `None` leaves `\pdfoutput` alone.
    pub output_format: Option<i32>,
    /// `-ini`: be INITEX (`ini_version`): no format is loaded unless the
    /// first line asks with `&name`; `\patterns` and `\dump` work.
    pub ini: bool,
    /// The default format (web2c's `dump_name`, from `-fmt` or the program
    /// name), without `.fmt`.
    pub dump_name: alloc::vec::Vec<u8>,
    /// web2c's `versionstring`, printed after the banner: `" (TeX Live
    /// 2026)"` plus the distribution's `--with-banner-add`.
    pub version_string: &'static [u8],
    /// kpathsea's `kpse_invocation_name`: the program as invoked
    /// (pdfTeX's warnings name it).
    pub invocation_name: alloc::vec::Vec<u8>,
    /// `-jobname`.
    pub job_name: Option<alloc::vec::Vec<u8>>,
    /// `-output-comment`: replaces the DVI preamble's date comment.
    pub output_comment: Option<alloc::vec::Vec<u8>>,
    /// `log_openout`: log `\openout` (web2c's `openout` feature; off for
    /// `tex`).
    pub log_openout: bool,
    /// Memoized macro calls (DESIGN.md §7.7 step 0).
    pub memo: bool,
    /// Remembered skips of conditional text (`skipcache.rs`).
    pub skip_cache: bool,
    /// Accelerations of expansion (DESIGN.md §7.13): a set of `FAST_*`
    /// bits, each off giving tex.web's path (the CLI's `PARTEX_FAST`).
    pub fast: u32,
}

/// `Params::fast`: runs of tokens absorbed from the token list read at
/// once (`bulk.rs`).
pub const FAST_BULK: u32 = 1;
/// `Params::fast`: e-TeX's registers above 255 read by kind and number
/// (`xregs.rs`).
pub const FAST_XREGS: u32 = 2;
/// `Params::fast`: all accelerations.
pub const FAST_ALL: u32 = FAST_BULK | FAST_XREGS;
/// The accelerations by name (the CLI's `PARTEX_FAST_<NAME>=0`).
pub const FAST_NAMES: &[(&str, u32)] = &[("BULK", FAST_BULK), ("XREGS", FAST_XREGS)];

partex_engine::persist_struct!(Params {
    flavor,
    mem_bot,
    main_memory,
    extra_mem_top,
    extra_mem_bot,
    pool_size,
    string_vacancies,
    pool_free,
    max_strings,
    strings_free,
    font_mem_size,
    font_max,
    trie_size,
    hyph_size,
    buf_size,
    nest_size,
    max_in_open,
    param_size,
    save_size,
    stack_size,
    dvi_buf_size,
    error_line,
    half_error_line,
    max_print_line,
    hash_extra,
    expand_depth,
    pdftex_bugs,
    eight_bit,
    translation,
    mltex,
    enctex,
    file_line_error_style,
    parse_first_line,
    halt_on_error,
    interaction,
    diagnostics,
    shell_escape,
    restricted_shell,
    etex,
    output_format,
    ini,
    dump_name,
    version_string,
    invocation_name,
    job_name,
    output_comment,
    log_openout,
    memo,
    skip_cache,
    fast
});

impl Default for Params {
    /// web2c's compiled-in defaults (`setup_bound_var` in §1332), used
    /// for every name `texmf.cnf` does not set.
    fn default() -> Self {
        Self {
            flavor: Flavor::Tex,
            etex: false,
            output_format: None,
            mem_bot: 0,
            main_memory: 250_000,
            extra_mem_top: 0,
            extra_mem_bot: 0,
            pool_size: 200_000,
            string_vacancies: 75_000,
            pool_free: 5000,
            max_strings: 15_000,
            strings_free: 100,
            font_mem_size: 100_000,
            font_max: 500,
            trie_size: 20_000,
            hyph_size: 659,
            buf_size: 3000,
            nest_size: 50,
            max_in_open: 15,
            param_size: 60,
            save_size: 4000,
            stack_size: 300,
            dvi_buf_size: 16_384,
            error_line: 79,
            half_error_line: 50,
            max_print_line: 79,
            hash_extra: 0,
            expand_depth: 10_000,
            pdftex_bugs: partex_engine::bugs::PdftexBugs::ALL,
            eight_bit: false,
            translation: None,
            mltex: false,
            enctex: false,
            file_line_error_style: false,
            parse_first_line: false,
            halt_on_error: false,
            interaction: None,
            diagnostics: true,
            shell_escape: false,
            restricted_shell: true,
            ini: false,
            dump_name: b"tex".to_vec(),
            version_string: b" (TeX Live 2026)",
            invocation_name: alloc::vec::Vec::from(*b"tex"),
            job_name: None,
            output_comment: None,
            log_openout: false,
            memo: false,
            skip_cache: true,
            fast: FAST_ALL,
        }
    }
}

impl Params {
    /// `texk/web2c/triptrap/texmf.cnf` (the only cnf file the trip test
    /// reads) on top of the compiled-in defaults.
    #[must_use]
    pub fn trip() -> Self {
        Self {
            mem_bot: 1,
            main_memory: 3000,
            error_line: 64,
            half_error_line: 32,
            max_print_line: 72,
            max_strings: 3000,
            string_vacancies: 8000,
            pool_size: 40_000,
            font_mem_size: 20_000,
            font_max: 75,
            stack_size: 200,
            nest_size: 40,
            buf_size: 500,
            save_size: 600,
            dvi_buf_size: 800,
            ..Self::default()
        }
    }
}

/// The engine being emulated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Flavor {
    /// Knuth's TeX (`tex`), as web2c builds it.
    #[default]
    Tex,
    /// pdfTeX (`pdftex`), which includes e-TeX.
    PdfTex,
}

partex_engine::persist_enum!(Flavor { Tex, PdfTex });
