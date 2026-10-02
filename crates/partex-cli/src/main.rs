//! `partex` command-line driver: `tex`-compatible options and a native
//! [`Host`](partex_core::Host).

mod bibtex;
mod cache;
mod clock;
mod compat;
mod config;
#[cfg(feature = "deps")]
mod deps;
mod dvithread;
mod eventlog;
mod events;
mod intervals;
mod lz;
mod machinehost;
mod makeindex;
mod modern;
mod native;
mod outline;
mod render;
mod resident;
mod sanitize;
mod session;
mod snippet;
mod store;
#[cfg(feature = "deps")]
mod texprof;
mod timeline;
mod tokendeps;
mod warnings;
mod zlib;

use partex_core::{Flavor, Params, PdftexBugs, Tex, Untracked};

/// The engine command line this invocation runs (see `compat.rs`): the
/// program name TeX sees, then its arguments.
/// (The modern command line sets it once per job it runs: a format, then
/// the document.)
static ARGS: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

/// The engine command line (empty before one is chosen).
pub fn args() -> Vec<String> {
    ARGS.read().map(|a| a.clone()).unwrap_or_default()
}

/// Whether this is the modern command line (`modern.rs`), which chooses
/// the engine command line itself.
static MODERN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Say that this is the modern command line.
pub fn set_modern() {
    MODERN.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Choose the engine command line `args` (program name first).
pub fn set_args(args: Vec<String>) {
    if let Ok(mut a) = ARGS.write() {
        *a = args;
    }
}

/// kpathsea for `tex`: `SELFAUTOLOC` is where the executable lives; if no
/// `texmf.cnf` is reachable from there (a development build), use the
/// location of `tex` on `PATH`, so partex sees TeX Live's configuration
/// (not partex itself, installed as `tex` in place of TeX).
fn kpse_instance(progname: &str, engine: &str) -> partex_kpse::Kpse {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.canonicalize().ok());
    let own = exe
        .as_ref()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf));
    if let Some(dir) = &own {
        let mut k = partex_kpse::Kpse::new(dir, progname, engine);
        if k.var_value("TEXMF").is_some() {
            return k;
        }
    }
    let tex_dir = std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join("tex"))
            .filter_map(|t| t.canonicalize().ok())
            .find(|t| t.is_file() && Some(t) != exe.as_ref())
            .and_then(|t| t.parent().map(std::path::Path::to_path_buf))
    });
    let dir = tex_dir.or(own).unwrap_or_else(|| "/usr/bin".into());
    partex_kpse::Kpse::new(&dir, progname, engine)
}

/// The engine a program name stands for: TeX Live's `etex`, `pdflatex`,
/// `latex`, … are pdfTeX under other names (the name picks the format).
fn flavor_of(progname: &str) -> Flavor {
    match progname {
        "pdftex" | "etex" | "pdfetex" | "pdflatex" | "latex" | "pdfcsplain" | "dvilualatex"
        | "mex" | "pdfmex" | "utf8mex" | "amstex" | "eplain" | "texsis" => Flavor::PdfTex,
        _ => Flavor::Tex,
    }
}

/// web2c's `atoi`: the leading decimal integer, 0 if none.
fn atoi(s: &[u8]) -> i64 {
    let s = std::str::from_utf8(s).unwrap_or("").trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let end = digits
        .bytes()
        .position(|c| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    let v = digits[..end].parse::<i64>().unwrap_or(0);
    if neg { -v } else { v }
}

/// §1332's `setup_bound_var`s (web2c's `setupboundvariable`): capacities
/// from the environment or `texmf.cnf`, over the compiled-in defaults.
fn setup_bound_vars(kpse: &mut partex_kpse::Kpse, p: &mut Params) {
    let vars: [(&str, &mut i32); 25] = [
        ("mem_bot", &mut p.mem_bot),
        ("main_memory", &mut p.main_memory),
        ("extra_mem_top", &mut p.extra_mem_top),
        ("extra_mem_bot", &mut p.extra_mem_bot),
        ("pool_size", &mut p.pool_size),
        ("string_vacancies", &mut p.string_vacancies),
        ("pool_free", &mut p.pool_free),
        ("max_strings", &mut p.max_strings),
        ("strings_free", &mut p.strings_free),
        ("font_mem_size", &mut p.font_mem_size),
        ("font_max", &mut p.font_max),
        ("trie_size", &mut p.trie_size),
        ("hyph_size", &mut p.hyph_size),
        ("buf_size", &mut p.buf_size),
        ("nest_size", &mut p.nest_size),
        ("max_in_open", &mut p.max_in_open),
        ("param_size", &mut p.param_size),
        ("save_size", &mut p.save_size),
        ("stack_size", &mut p.stack_size),
        ("dvi_buf_size", &mut p.dvi_buf_size),
        ("error_line", &mut p.error_line),
        ("half_error_line", &mut p.half_error_line),
        ("max_print_line", &mut p.max_print_line),
        ("hash_extra", &mut p.hash_extra),
        ("expand_depth", &mut p.expand_depth),
    ];
    for (name, var) in vars {
        if let Some(v) = kpse.var_value(name) {
            let v = atoi(&v);
            if v < 0 || (v == 0 && *var > 0) {
                eprintln!(
                    "partex: Bad value ({v}) in environment or texmf.cnf for {name}, keeping {var}."
                );
            } else {
                *var = i32::try_from(v).unwrap_or(i32::MAX);
            }
        }
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: partex [-engine=tex|pdftex] [-ini] [-etex] [-interaction=MODE] [-jobname=NAME] [-output-comment=S] [-shell-escape|-shell-restricted|-no-shell-escape] [-watch [-checkpoint-every=N]] [-resident] [-converge] ARGS..."
    );
    std::process::exit(2);
}

/// texmf.cnf's settings (web2c's `setup_bound_variables` and
/// `texmf_yesno`), with the command line's `shell` mode.
fn configure(kpse: &mut partex_kpse::Kpse, params: &mut Params, shell: Option<(bool, bool)>) {
    setup_bound_vars(kpse, params);
    // texmfmp.c: the command line, else `shell_escape` (`t`, `y`, `1`:
    // enabled; `p`: restricted)
    (params.shell_escape, params.restricted_shell) = shell.unwrap_or_else(|| {
        match kpse
            .var_value("shell_escape")
            .and_then(|v| v.first().copied())
        {
            Some(b't' | b'y' | b'1') => (true, false),
            Some(b'p') => (true, true),
            _ => (false, false),
        }
    });
    params.log_openout = kpse
        .var_value("log_openout")
        .is_some_and(|v| matches!(v.first(), Some(b't' | b'y' | b'1')));
}

/// The options that take a value, which web2c's `getopt` also accepts as
/// the next argument.
const VALUE_OPTIONS: &[&str] = &[
    "checkpoint-every",
    "default-translate-file",
    "engine",
    "fmt",
    "interaction",
    "jobname",
    "output-comment",
    "output-directory",
    "output-format",
    "progname",
    "translate-file",
];

/// The arguments with each option's value joined to it (`-fmt x` is
/// `-fmt=x`), up to the first one that is not an option.
fn joined_args(args: impl Iterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut args = args.peekable();
    while let Some(a) = args.next() {
        let opt = a.strip_prefix("--").or_else(|| a.strip_prefix('-'));
        match opt {
            Some(o) if VALUE_OPTIONS.contains(&o) => {
                let v = args.next().unwrap_or_else(|| usage());
                out.push(format!("-{o}={v}"));
            }
            Some(_) => out.push(a),
            None => {
                out.push(a);
                out.extend(args);
                break;
            }
        }
    }
    out
}

/// texmfmp.c's `readtcxfile`: the TCX file `name` (`.tcx` added if it
/// has no suffix), looked up along the web2c path.
fn read_tcx(kpse: &mut partex_kpse::Kpse, name: &str) -> Option<partex_core::Translation> {
    let base = name.rsplit('/').next().unwrap_or(name);
    let name = if base.contains('.') {
        name.to_owned()
    } else {
        format!("{name}.tcx")
    };
    let found = kpse.find_file(name.as_bytes(), partex_kpse::Format::Web2c, true)?;
    let text = std::fs::read(native::path(&found)).ok()?;
    let (t, warnings) = partex_core::Translation::parse(&found, &text);
    for w in warnings {
        eprintln!("{}", String::from_utf8_lossy(&w));
    }
    Some(t)
}

/// texmfmp.c's `parse_first_line`: the format a `%&name` first line of
/// the main input file (`arg`, when it is a file name) names, if there
/// is such a format.
fn first_line_format(kpse: &mut partex_kpse::Kpse, arg: Option<&String>) -> Option<String> {
    let arg = arg.filter(|a| !a.starts_with('&') && !a.starts_with('\\'))?;
    let found = kpse.find_file(arg.as_bytes(), partex_kpse::Format::Tex, false)?;
    let text = std::fs::read(native::path(&found)).ok()?;
    let line = text.split(|&c| c == b'\n').next()?;
    let rest = line.strip_prefix(b"%&")?;
    let rest = &rest[rest
        .iter()
        .take_while(|c| matches!(c, b' ' | b'\t'))
        .count()..];
    let name = rest
        .split(|&c| c == b' ')
        .next()
        .filter(|n| !n.is_empty() && n[0] != b'-')?;
    let mut fmt = name.to_vec();
    fmt.extend_from_slice(b".fmt");
    kpse.find_file(&fmt, partex_kpse::Format::Fmt, false)?;
    String::from_utf8(name.to_vec()).ok()
}

/// The options that only set a parameter from their value; false if `o`
/// is none of them.
fn set_option(params: &mut Params, o: &str) -> bool {
    match o {
        o if o.starts_with("output-format=") => {
            params.output_format = Some(match &o[14..] {
                "dvi" => 0,
                "pdf" => 1,
                _ => usage(),
            });
        }
        o if o.starts_with("interaction=") => {
            params.interaction = Some(match &o[12..] {
                "batchmode" => 0,
                "nonstopmode" => 1,
                "scrollmode" => 2,
                "errorstopmode" => 3,
                _ => usage(),
            });
        }
        o if o.starts_with("jobname=") => params.job_name = Some(o[8..].into()),
        o if o.starts_with("output-comment=") => {
            params.output_comment = Some(o[15..].into());
        }
        _ => return false,
    }
    true
}

/// The command line, as web2c reads it.
struct CommandLine {
    params: Params,
    /// The arguments after the options (TeX's first line).
    rest: Vec<String>,
    engine: Flavor,
    progname: String,
    watch: bool,
    /// web2c's `shellenabledp` and `restrictedshell`, if given.
    shell: Option<(bool, bool)>,
    parse_first_line: Option<bool>,
    checkpoint_every: u64,
    user_progname: Option<String>,
    fmt_name: Option<String>,
    translate: Option<String>,
    output_dir: Option<Vec<u8>>,
}

fn parse_command_line() -> CommandLine {
    let mut params = Params {
        // pdfTeX §1328: web2c's build-specific version string enters the banner.
        version_string: env!("PARTEX_ORACLE_VERSION_SUFFIX").as_bytes(),
        // (`PARTEX_SKIPCACHE=0`: skip conditional text as tex.web does)
        skip_cache: std::env::var_os("PARTEX_SKIPCACHE").is_none_or(|v| v != "0"),
        fast: fast_switches(),
        pdftex_bugs: pdftex_bugs(),
        ..Params::default()
    };
    let mut rest: Vec<String> = Vec::new();
    // web2c: the program name is how the binary was invoked (or the
    // engine `--compat` names).
    let invoked = args().first().cloned().unwrap_or_else(|| "tex".into());
    let mut engine = flavor_of(&invoked);
    let mut progname = invoked;
    let mut watch = false;
    let mut shell = None;
    let mut parse_first_line = None;
    let mut checkpoint_every = 100; // lines
    let mut user_progname = None;
    let mut fmt_name = None;
    let mut translate = None;
    let mut default_translate = None;
    let mut output_dir =
        std::env::var_os("TEXMF_OUTPUT_DIRECTORY").map(std::ffi::OsString::into_encoded_bytes);
    for a in joined_args(args().into_iter().skip(1)) {
        let opt = a.strip_prefix("--").or_else(|| a.strip_prefix('-'));
        match opt {
            _ if !rest.is_empty() => rest.push(a),
            Some("version") => {
                println!("partex {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            Some("ini") => params.ini = true,
            Some("8bit") => params.eight_bit = true,
            Some(o) if o.starts_with("translate-file=") => translate = Some(o[15..].to_owned()),
            Some(o) if o.starts_with("default-translate-file=") => {
                default_translate = Some(o[23..].to_owned());
            }
            Some(o) if o.starts_with("output-directory=") => {
                output_dir = Some(o.as_bytes()[17..].to_vec());
            }
            Some("watch") => watch = true,
            // (asked for before the command line is read: `resident.rs`,
            // `converge_wanted`)
            Some("resident" | "converge") => {}
            Some(o) if o.starts_with("checkpoint-every=") => {
                checkpoint_every = o[17..].parse().unwrap_or_else(|_| usage());
            }
            Some(o) if o.starts_with("fmt=") => {
                params.dump_name = o[4..].into();
                fmt_name = Some(o[4..].to_owned());
            }
            Some(o) if o.starts_with("progname=") => user_progname = Some(o[9..].to_owned()),
            Some(o) if o.starts_with("engine=") => {
                engine = match &o[7..] {
                    "tex" => Flavor::Tex,
                    "pdftex" => Flavor::PdfTex,
                    _ => usage(),
                };
                if progname == "tex" && engine == Flavor::PdfTex {
                    "pdftex".clone_into(&mut progname);
                }
            }
            Some("etex") => params.etex = true,
            Some(o) if set_option(&mut params, o) => {}
            Some("file-line-error") => params.file_line_error_style = true,
            Some("halt-on-error") => params.halt_on_error = true,
            // (partex runs no commands: `\write18` is always denied, but
            // TeX reports the mode it was given)
            Some("shell-escape") => shell = Some((true, false)),
            Some("shell-restricted") => shell = Some((true, true)),
            Some("no-shell-escape") => shell = Some((false, false)),
            Some("parse-first-line") => parse_first_line = Some(true),
            Some("no-parse-first-line") => parse_first_line = Some(false),
            Some(_) => usage(),
            None => rest.push(a),
        }
    }
    CommandLine {
        params,
        rest,
        engine,
        progname,
        watch,
        shell,
        parse_first_line,
        checkpoint_every,
        user_progname,
        fmt_name,
        translate: translate.or(default_translate),
        output_dir,
    }
}

/// The pdfTeX bugs to reproduce: all, unless `PARTEX_PDFTEX_BUGS` says
/// otherwise (`none`, or `-name,...`; see `partex_engine::bugs`).
fn pdftex_bugs() -> PdftexBugs {
    let Ok(spec) = std::env::var("PARTEX_PDFTEX_BUGS") else {
        return PdftexBugs::ALL;
    };
    PdftexBugs::parse(&spec).unwrap_or_else(|name| {
        eprintln!("partex: PARTEX_PDFTEX_BUGS: unknown pdfTeX bug `{name}`; known:");
        for (n, what) in PdftexBugs::NAMES {
            eprintln!("  {n}: {what}");
        }
        std::process::exit(1)
    })
}

/// The accelerations of expansion to use (DESIGN.md §7.13): all, unless
/// `PARTEX_FAST=0` (none) or `PARTEX_FAST_<NAME>=0` turns one off.
fn fast_switches() -> u32 {
    use partex_core::params::{FAST_ALL, FAST_NAMES};
    let off = |name: &str| std::env::var_os(name).is_some_and(|v| v == "0");
    if off("PARTEX_FAST") {
        return 0;
    }
    let mut fast = FAST_ALL;
    for &(name, bit) in FAST_NAMES {
        if off(&format!("PARTEX_FAST_{name}")) {
            fast &= !bit;
        }
    }
    fast
}

/// A TeX job as the engine command line (`args()`) sets it up.
struct Job {
    pub params: Params,
    /// TeX's first line.
    pub command_line: String,
    pub host: native::NativeHost,
    pub watch: bool,
    pub checkpoint_every: u64,
}

/// The job the engine command line asks for: its parameters, kpathsea
/// and `texmf.cnf` settings, and host.
fn setup() -> Job {
    let CommandLine {
        mut params,
        rest,
        engine,
        mut progname,
        watch,
        shell,
        parse_first_line,
        checkpoint_every,
        user_progname,
        fmt_name,
        translate,
        output_dir,
    } = parse_command_line();
    let command_line = rest.join(" ");
    params.invocation_name = progname.clone().into_bytes();
    let format_given = fmt_name.is_some() || user_progname.is_some();
    // texmfmp.c: `-fmt` names the program for kpathsea unless `-progname`
    // does.
    if let Some(p) = user_progname.or(fmt_name) {
        progname = p;
    }
    params.flavor = engine;
    // web2c: the default format is named after the program (`-fmt` wins).
    if params.dump_name == b"tex" {
        params.dump_name = progname.clone().into_bytes();
    }
    let engine_name = match engine {
        Flavor::Tex => "tex",
        Flavor::PdfTex => "pdftex",
    };
    let mut kpse = kpse_instance(&progname, engine_name);
    // texmfmp.c: `parse_first_line`: a `%&name` first line of the main
    // input file names the format, unless one was given.
    params.parse_first_line = parse_first_line.unwrap_or_else(|| {
        kpse.var_value("parse_first_line")
            .is_some_and(|v| matches!(v.first(), Some(b't' | b'y' | b'1')))
    });
    if params.parse_first_line
        && !format_given
        && let Some(name) = first_line_format(&mut kpse, rest.first())
    {
        progname.clone_from(&name);
        params.dump_name = name.into_bytes();
        kpse = kpse_instance(&progname, engine_name);
    }
    configure(&mut kpse, &mut params, shell);
    params.translation = translate.and_then(|name| read_tcx(&mut kpse, &name));
    let mut host = native::NativeHost::new(
        clock::Clock::from_env(params.flavor == partex_core::params::Flavor::Tex),
        kpse,
    );
    host.output_dir = output_dir;
    if watch || converge_wanted() {
        make_output_dir(&host);
    }
    // (`PARTEX_DVI_THREAD=0` writes the DVI file inline, for comparison)
    host.dvi_thread = std::env::var_os("PARTEX_DVI_THREAD").is_none_or(|v| v != "0");
    Job {
        params,
        command_line,
        host,
        watch,
        checkpoint_every,
    }
}

/// Bare `partex` is the modern command line; under an engine's name, or
/// with `--compat`, partex is that engine (`compat.rs`).
fn choose_command_line() {
    let mut raw = std::env::args();
    let argv0 = raw.next().unwrap_or_default();
    let compat_env = std::env::var("PARTEX_COMPAT").ok();
    match compat::select(&argv0, raw.collect(), compat_env.as_deref()) {
        compat::Selection::Compat { progname, mut args } => {
            args.insert(0, progname);
            set_args(args);
        }
        compat::Selection::Modern(args) if args.first().is_some_and(|a| a == "outline") => {
            outline::main(&args[1..])
        }
        compat::Selection::Modern(args) => modern::main(&args),
        compat::Selection::Unsupported(name) => {
            eprintln!("partex: {name} is not supported yet");
            std::process::exit(2);
        }
    }
}

fn main() {
    if std::env::var_os("PARTEX_LIST_HASH").is_some_and(|v| v == "0") {
        partex_core::statehash::LIST_HASHES.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    // (macro arguments' lists kept for the next argument: `tok.rs`)
    if std::env::var_os("PARTEX_PARAM_ARENA").is_some_and(|v| v == "0") {
        partex_core::set_arg_pool(false);
    }
    // (a restore rebases the running engine's vectors: `journal.rs`)
    if std::env::var_os("PARTEX_RESTORE_REBASE").is_some_and(|v| v == "0") {
        partex_core::set_rebase(false);
    }
    // (a snapshot compares eqtb's blocks written: `journal.rs`)
    if std::env::var_os("PARTEX_JVEC_BLOCKS").is_some_and(|v| v == "0") {
        partex_core::set_written_blocks(false);
    }
    // (a snapshot's vectors by their live prefix: `flat.rs`)
    if std::env::var_os("PARTEX_FLAT_LIVE").is_some_and(|v| v == "0") {
        partex_core::set_flat_live(false);
    }
    // (a snapshot visits the token lists changed by their ids: `tok.rs`)
    if std::env::var_os("PARTEX_TOK_CHANGED").is_some_and(|v| v == "0") {
        partex_core::set_changed_ids(false);
    }
    if bibtex::wanted() {
        bibtex::main();
    }
    if makeindex::wanted() {
        makeindex::main();
    }
    choose_command_line();
    if resident::wanted()
        && let Some(status) = resident::client(&resident::socket_path())
    {
        std::process::exit(status);
    }
    let Job {
        params,
        command_line,
        host,
        watch,
        checkpoint_every,
    } = setup();
    if watch {
        watch_job(params, command_line.into_bytes(), host, checkpoint_every);
    }
    if sanitize::wanted() {
        let history = sanitize::run(host, params, command_line.as_bytes());
        std::process::exit(i32::from(history > 1));
    }
    if let Some(path) = resident::serving() {
        resident::serve(
            params,
            command_line.into_bytes(),
            host,
            checkpoint_every,
            &path,
        );
    }
    if converge_wanted() {
        converge_job(params, command_line.into_bytes(), host, checkpoint_every);
    }
    #[cfg(feature = "deps")]
    if let Some(path) = std::env::var_os("PARTEX_TEXPROF") {
        let mut tex = Tex::new(host, texprof::Profiler::default(), params);
        let history = tex.run(command_line.as_bytes());
        texprof::report(&tex, std::path::Path::new(&path));
        std::process::exit(i32::from(history > 1));
    }
    #[cfg(feature = "deps")]
    if let Some(path) = std::env::var_os("PARTEX_DEPS") {
        let filter = std::env::var("PARTEX_DEPS_MATCH").unwrap_or_default();
        let base = std::env::var("PARTEX_DEPS_BASE").unwrap_or_default();
        let mut tex = Tex::new(host, deps::Recorder::new(filter, base), params);
        let history = tex.run(command_line.as_bytes());
        deps::report(&tex, std::path::Path::new(&path));
        std::process::exit(i32::from(history > 1));
    }
    // (`PARTEX_EVENTS=DIR`: the event log of a plain build, `eventlog.rs`)
    if let Some(dir) = std::env::var_os("PARTEX_EVENTS") {
        let mut tex = Tex::new(host, eventlog::Log::default(), params);
        let history = tex.run(command_line.as_bytes());
        if let Err(e) = eventlog::write(&tex, std::path::Path::new(&dir)) {
            eprintln!("partex: the event log: {e}");
        }
        std::process::exit(i32::from(history > 1));
    }
    if std::env::var("PARTEX_MACHINE").is_ok_and(|v| v == "1") {
        let history = machinehost::run(host, params, command_line.as_bytes());
        std::process::exit(i32::from(history > 1));
    }
    if std::env::var("PARTEX_SSA").is_ok_and(|v| v == "1") {
        let history = run_ssa(host, params, command_line.as_bytes());
        std::process::exit(i32::from(history > 1));
    }
    let history = {
        let _p = timeline::phase("run");
        run_memo(host, params, command_line.as_bytes())
    };
    timeline::finish();
    // web2c's `do_final_end`: exit status 1 for errors or worse.
    std::process::exit(i32::from(history > 1));
}

/// `-watch`: build, then rebuild incrementally whenever a file the job
/// read changes (or a line arrives on standard input; `q` quits). Terminal
/// input is off, so the default interaction is `nonstopmode`.
fn watch_job(mut params: Params, command_line: Vec<u8>, host: native::NativeHost, every: u64) -> ! {
    use std::io::Write;
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime};

    if params.interaction.is_none() {
        params.interaction = Some(1);
    }
    let mut s = session::Session::new(params, command_line, host, every);
    // Each build runs to its fixpoint, with BibTeX and makeindex between
    // passes, as `-converge` does.
    let mut between = Between::default();
    let mut serve = |s: &mut session::Session, history: Option<i32>| -> i32 {
        let (reports, term, h) = serve_request(s, history, true, &mut between);
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(&term);
        let _ = writeln!(out);
        let _ = out.flush();
        for r in reports {
            eprintln!("{r}");
        }
        h
    };
    let mut history = serve(&mut s, None);
    // The document's own files (not absolute paths: the TeX tree's) are
    // looked at every `PARTEX_WATCH_POLL_MS` (20), the rest every tenth
    // time: an edit is seen within a few milliseconds, for a few hundred
    // `stat`s a second.
    let poll = Duration::from_millis(
        std::env::var("PARTEX_WATCH_POLL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20),
    );
    let split = |s: &session::Session| -> (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>) {
        s.inputs().into_iter().partition(|p| !p.is_absolute())
    };
    let stamps = |files: &[std::path::PathBuf]| -> Vec<Option<SystemTime>> {
        files
            .iter()
            .map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
            .collect()
    };
    let (mut local, mut tree) = split(&s);
    let mut last = (stamps(&local), stamps(&tree));
    let mut tick = 0u32;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        let asked = match rx.recv_timeout(poll) {
            Ok(line) if line.trim() == "q" => std::process::exit(0),
            Ok(line) => Some(line),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // (no more commands: keep watching the files)
                std::thread::sleep(poll);
                None
            }
        };
        tick = tick.wrapping_add(1);
        let changed =
            stamps(&local) != last.0 || (tick.is_multiple_of(10) && stamps(&tree) != last.1);
        if asked.is_some() || changed {
            history = serve(&mut s, Some(history));
            (local, tree) = split(&s);
            last = (stamps(&local), stamps(&tree));
        }
        if let Some(line) = asked {
            // (lets a driver tell which request a report answers)
            eprintln!("partex: done {}", line.trim());
        }
    }
}

/// `-converge` (or `PARTEX_CONVERGE=1`): run the job to its fixpoint.
/// A build that replaces latexmk (`-watch`, `-converge`) makes its output
/// directory as latexmk does; a plain run leaves it to TeX, which says it
/// can't write.
fn make_output_dir(host: &native::NativeHost) {
    if let Some(d) = &host.output_dir {
        let _ = std::fs::create_dir_all(native::path(d));
    }
}

fn converge_wanted() -> bool {
    std::env::var_os("PARTEX_CONVERGE").is_some_and(|v| !v.is_empty() && v != "0")
        || args()
            .iter()
            .skip(1)
            .any(|a| a == "-converge" || a == "--converge")
}

/// Passes of a converging build at most (LaTeX settles in two or three).
const PASSES: usize = 5;

/// The programs a converging build runs between its passes, as the
/// conventional pipeline does (BibTeX, makeindex), with what they remember
/// across passes (and a resident session's requests).
#[derive(Default)]
pub struct Between {
    bib: bibtex::Runs,
    idx: makeindex::Runs,
}

/// Build (the first time: `history` is `None`) or rebuild, and write the
/// outputs; with `converge`, rebuild again while a file the job read has
/// changed since (its own `.aux`, `.toc` and the like: the passes one
/// would rerun by hand, each resuming from the checkpoint before the
/// first changed read). The reports, the last terminal transcript, and
/// the history.
fn serve_request(
    s: &mut session::Session,
    history: Option<i32>,
    converge: bool,
    between: &mut Between,
) -> (Vec<String>, Vec<u8>, i32) {
    serve_observed(s, history, converge, between, &mut |_| {})
}

/// [`serve_request`], telling `observe` what it does as it goes.
fn serve_observed(
    s: &mut session::Session,
    history: Option<i32>,
    converge: bool,
    between: &mut Between,
    observe: &mut dyn FnMut(events::Progress),
) -> (Vec<String>, Vec<u8>, i32) {
    let mut reports = Vec::new();
    let mut h = history.unwrap_or(0);
    observe(events::Progress::PassStart(1));
    let mut r = match history {
        None => Some(("built", s.build())),
        Some(_) => s.rebuild().map(|r| ("rebuilt", r)),
    };
    let mut passes = 1;
    loop {
        observe(events::Progress::Pass(passes, r.as_ref().map(|r| &r.1)));
        match &r {
            Some((what, rep)) => {
                reports.push(report_line(s, rep, what));
                h = rep.history;
            }
            None => reports.push(String::from("partex: unchanged")),
        }
        let term = match s.write_outputs() {
            Ok(term) => {
                if std::env::var_os("PARTEX_PASS_DUMP").is_some() {
                    for ext in [&b".aux"[..], b".toc", b".out", b".log"] {
                        for (name, bytes) in s.outputs_ending(ext) {
                            let n = format!("{}.pass{passes}", String::from_utf8_lossy(&name));
                            let _ = std::fs::write(n, bytes);
                        }
                    }
                }
                term
            }
            Err(e) => {
                reports.push(format!("partex: writing the outputs failed: {e}"));
                return (reports, Vec::new(), 3);
            }
        };
        if converge && r.is_some() {
            // (BibTeX and makeindex between passes, as the conventional
            // pipeline runs them)
            let mut tools = bibtex::after_pass(&mut between.bib, &s.outputs_ending(b".aux"));
            tools.extend(makeindex::after_pass(
                &mut between.idx,
                &s.outputs_ending(b".idx"),
            ));
            for t in &tools {
                observe(events::Progress::Tool(t));
            }
            reports.extend(tools);
        }
        if !converge || r.is_none() || passes == PASSES {
            return (reports, term, h);
        }
        passes += 1;
        observe(events::Progress::PassStart(passes));
        r = s.rebuild().map(|r| ("converged a pass", r));
    }
}

/// `-converge`: build, rebuilding while the job's own files change, and
/// exit as the last pass would. (A session without a watcher: terminal
/// input is off, so the default interaction is `nonstopmode`.)
fn converge_job(
    mut params: Params,
    command_line: Vec<u8>,
    host: native::NativeHost,
    every: u64,
) -> ! {
    use std::io::Write;
    if params.interaction.is_none() {
        params.interaction = Some(1);
    }
    let mut s = session::Session::new(params, command_line, host, every);
    let (reports, term, h) = converge_saved(&mut s, &mut Between::default(), &mut |_| {});
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(&term);
    let _ = out.flush();
    if std::env::var_os("PARTEX_REPORT").is_some() {
        for r in reports {
            eprintln!("{r}");
        }
    }
    timeline::finish();
    std::process::exit(i32::from(h > 1));
}

/// Run session `s` to its fixpoint, going on from the session a previous
/// process saved for the same job (and saving it again): the reports,
/// the terminal transcript and the history.
fn converge_saved(
    s: &mut session::Session,
    between: &mut Between,
    observe: &mut dyn FnMut(events::Progress),
) -> (Vec<String>, Vec<u8>, i32) {
    let key = saved_session_key();
    let t0 = std::time::Instant::now();
    let loaded = {
        let _p = timeline::phase("load session");
        key.and_then(cache::get).is_some_and(|saved| s.load(saved))
    };
    let mut reports = Vec::new();
    if loaded {
        reports.push(format!(
            "partex: loaded a saved session in {:.1} ms: {} checkpoints",
            t0.elapsed().as_secs_f64() * 1e3,
            s.checkpoints()
        ));
    }
    let history = loaded.then(|| s.history());
    let (more, term, h) = serve_observed(s, history, true, between, observe);
    reports.extend(more);
    if let Some(key) = key
        && s.changed()
    {
        let t0 = std::time::Instant::now();
        let saved = {
            let _p = timeline::phase("save session");
            s.save()
        };
        if let Some(saved) = saved {
            let t1 = std::time::Instant::now();
            {
                let _p = timeline::phase("write session");
                cache::put(key, &saved);
            }
            if std::env::var_os("PARTEX_WATCH_DEBUG").is_some() {
                eprintln!(
                    "partex: save: written in {:.1} ms",
                    t1.elapsed().as_secs_f64() * 1e3
                );
            }
            reports.push(format!(
                "partex: saved the session in {:.1} ms: {} bytes",
                t0.elapsed().as_secs_f64() * 1e3,
                saved.len()
            ));
        }
    }
    (reports, term, h)
}

/// Where `-converge` keeps its session between processes (DESIGN.md
/// §5.3): by directory, arguments and environment, like a resident
/// session; and like one it keeps its clock for the day unless
/// `SOURCE_DATE_EPOCH` and `FORCE_SOURCE_DATE=1` fix TeX's dates.
/// `None` with `PARTEX_PERSIST=0` (or no cache).
fn saved_session_key() -> Option<u128> {
    if std::env::var_os("PARTEX_PERSIST").is_some_and(|v| v == "0") {
        return None;
    }
    let fixed = std::env::var_os("SOURCE_DATE_EPOCH").is_some()
        && std::env::var_os("FORCE_SOURCE_DATE").is_some_and(|v| v == "1");
    let day = (!fixed).then(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() / 86_400)
    });
    let env: Vec<_> = resident::job_env()
        .iter()
        .map(|(k, v)| (k.as_encoded_bytes().to_vec(), v.as_encoded_bytes().to_vec()))
        .collect();
    // (the raw arguments name the binary; the modern command line's
    // subcommand and options are not the job's, the engine command line it
    // resolved is)
    let raw: Vec<Vec<u8>> = if MODERN.load(std::sync::atomic::Ordering::Relaxed) {
        let exe = std::env::current_exe().unwrap_or_default();
        vec![exe.into_os_string().into_encoded_bytes()]
    } else {
        std::env::args_os()
            .map(|a| a.as_encoded_bytes().to_vec())
            .collect()
    };
    let args: Vec<_> = raw
        .into_iter()
        .chain(args().iter().map(|a| a.as_bytes().to_vec()))
        .collect();
    let dir = std::env::current_dir().ok()?;
    Some(partex_core::persist_hash(&(
        "session/2",
        dir.as_os_str().as_encoded_bytes(),
        args,
        env,
        day,
    )))
}

/// What a session's build did, as `-watch` reports it.
fn report_line(s: &session::Session, r: &session::Report, what: &str) -> String {
    format!(
        "partex: {what} in {:.1} ms: {} of {} commands run, {} checkpoints{}{}",
        r.elapsed.as_secs_f64() * 1e3,
        r.commands,
        r.total_commands,
        s.checkpoints(),
        r.cut_at
            .map(|c| format!(", converged at command {c}"))
            .unwrap_or_default(),
        if r.history > 1 { " (with errors)" } else { "" }
    ) + &r.why.iter().fold(String::new(), |mut s, w| {
        s.push_str("\npartex:   ");
        s.push_str(w);
        s
    })
}

/// Runs without a tracker, with memoized macro calls as `PARTEX_MEMO` asks.
/// Link the effects `tex` made (`PARTEX_EFFECTS=1`) and hand the files
/// and terminal text to its host.
fn deliver_effects<T: partex_core::track::Tracker>(tex: &mut Tex<native::NativeHost, T>) {
    use partex_core::host::{Host, WriteId};
    let fx = tex.take_effects();
    let threads = partex_incr::Threads::available();
    let host = tex.host_mut();
    let linked = {
        let _p = timeline::phase("link");
        partex_core::effects::link(&[&fx], &threads, &mut |level, data| {
            crate::zlib::deflate_stream(level, data)
        })
    };
    match linked {
        Ok(l) => {
            for (id, bytes) in &l.files {
                host.write(WriteId(*id), bytes);
            }
            for id in l.closed {
                host.close(id);
            }
            host.term_write(&l.term);
            for d in &l.diagnostics {
                host.diagnostic(d);
            }
            for c in &l.pages {
                host.shipping(*c);
            }
        }
        Err(e) => {
            eprintln!("partex: the link step failed: {e:?}");
            std::process::exit(3);
        }
    }
}

/// `PARTEX_SSA=1`: the build on the dynamic-SSA runtime (DESIGN.md
/// §7.17, `partex_core::ssa`), with `PARTEX_SSA_CHECK=1` check mode.
/// `PARTEX_SSA_REBUILD=<shell command>` runs the command after the cold
/// build (an edit) and builds again with the same records;
/// `PARTEX_SSA_TRACE=<file>` writes the last build's trace there.
fn run_ssa(host: native::NativeHost, params: Params, command_line: &[u8]) -> i32 {
    use partex_core::ssa::{Recorder, SsaTracker};
    let check = std::env::var("PARTEX_SSA_CHECK").is_ok_and(|v| v == "1");
    let rebuild = std::env::var("PARTEX_SSA_REBUILD").ok();
    let apply = std::env::var("PARTEX_SSA_APPLY").is_ok_and(|v| v == "1");
    let mut tex = Tex::new(host, SsaTracker::new(Recorder::new()), params);
    let t0 = std::time::Instant::now();
    let r = partex_core::ssa::run_applying(&mut tex, command_line, check, 0, apply);
    if check {
        eprintln!("partex: ssa lost writes {}", tex.lost_writes());
    }
    let millis = t0.elapsed().as_secs_f64() * 1e3;
    let t1 = std::time::Instant::now();
    let mut linker = SsaLinker::default();
    let how = linker.link(&mut tex);
    let link_ms = t1.elapsed().as_secs_f64() * 1e3;
    eprintln!("partex: ssa build 0: link {link_ms:.1} ms: {how}");
    let mut history = r.history;
    {
        let rec = tex.tracker().rec.borrow();
        report_read_counts(&rec);
        let s = rec.rt.stats;
        eprintln!(
            "partex: ssa build 0: {:.1} ms, calls {} (hits {}, misses {}, fresh {}), \
             paragraphs {} (hits {}), tokenize {} (hits {}), reads verified {}, \
             commands {} (in hit paragraphs {}), records {}, hits applied {} \
             (commands skipped {})",
            millis,
            s.hits + s.misses,
            s.hits,
            s.misses,
            s.fresh,
            r.paragraphs,
            r.para_hits,
            r.tokenize_calls,
            r.tokenize_hits,
            s.reads_verified,
            r.commands,
            r.commands_in_hits,
            rec.rt.live_records(),
            r.applied,
            r.commands_skipped,
        );
        report_routines("build 0", &r.routines);
        if check {
            report_check(&r, &rec);
        }
    }
    // (one rebuild after each line's command)
    for (n, cmd) in rebuild.as_deref().unwrap_or_default().lines().enumerate() {
        match rebuild_ssa(&mut tex, &mut linker, n + 1, cmd) {
            Ok(Some(h)) => history = h,
            Ok(None) => {}
            Err(code) => return code,
        }
    }
    if let Some(path) = std::env::var_os("PARTEX_SSA_TRACE") {
        let _ = std::fs::write(path, tex.tracker().rec.borrow().rt.trace().to_text());
    }
    history
}

/// Rebuild `n` of an SSA build (DESIGN 7.17.3): run the edit `cmd`, then
/// rebuild the same engine in place and link its files; the exit code it
/// sets, if its edits changed the job's (`Err`: it failed or stopped).
fn rebuild_ssa(
    tex: &mut Tex<native::NativeHost, partex_core::ssa::SsaTracker>,
    linker: &mut SsaLinker,
    n: usize,
    cmd: &str,
) -> Result<Option<i32>, i32> {
    let ok = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("partex: ssa: the rebuild command failed");
        return Err(3);
    }
    // (the same engine, rebuilt in place: DESIGN 7.17.3)
    let before = tex.tracker().rec.borrow().rt.stats;
    let routines = tex.tracker().rec.borrow().st.routines;
    let t0 = std::time::Instant::now();
    let trace = std::env::var("PARTEX_SSA_REBUILD_TRACE").is_ok_and(|v| v == "1");
    // (hits applied in the steps run, DESIGN 7.17.3: on unless `=0`)
    let apply = !matches!(std::env::var("PARTEX_SSA_APPLY").as_deref(), Ok("0"));
    let rr = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        partex_core::ssa::rebuild(tex, trace, apply)
    })) {
        Ok(rr) => rr,
        Err(e) => {
            for l in partex_core::ssa::rebuild_log(tex) {
                eprintln!("partex: ssa rebuild {n}: {l}");
            }
            std::panic::resume_unwind(e);
        }
    };
    for l in &rr.log {
        eprintln!("partex: ssa rebuild {n}: {l}");
    }
    let millis = t0.elapsed().as_secs_f64() * 1e3;
    let t1 = std::time::Instant::now();
    let how = linker.link(tex);
    let link_ms = t1.elapsed().as_secs_f64() * 1e3;
    eprintln!("partex: ssa rebuild {n}: link: {how}");
    let s = tex.tracker().rec.borrow().rt.stats;
    eprintln!(
        "partex: ssa rebuild {n}: {:.1} ms (the rebuild {millis:.1} ms, the link {link_ms:.1} ms), \
         edits {}, seeds {} \
         (loads of a changed φ {}, of a changed store {}, queries answered anew {}; data edited in it {}), \
         steps run {} (new {}, runs dropped {}, passed over {}), calls {} (fresh {}, \
         hits applied {} for {} commands), \
         definitions changed {}, readers marked {}, reads checked {}, positioned {}, \
         restored {}, from the format {}, commands {}",
        millis + link_ms,
        rr.edits,
        rr.seeds,
        rr.phi,
        rr.store_readers,
        rr.queries,
        rr.data_edits,
        rr.steps_run,
        rr.new_steps,
        rr.retries,
        rr.removed,
        s.hits + s.misses - before.hits - before.misses,
        s.fresh - before.fresh,
        rr.applied,
        rr.skipped,
        rr.defs_changed,
        rr.readers_marked,
        rr.reads_checked,
        rr.positioned,
        rr.restored,
        rr.initial,
        rr.commands,
    );
    {
        // (the rebuild's own calls, by routine)
        let now = tex.tracker().rec.borrow().st.routines;
        let d: Vec<partex_core::ssa::RoutineCount> = now
            .iter()
            .zip(&routines)
            .map(|(a, b)| partex_core::ssa::RoutineCount {
                calls: a.calls - b.calls,
                hits: a.hits - b.hits,
                records: a.records - b.records,
            })
            .collect();
        report_routines(&format!("rebuild {n}"), &d);
    }
    if let Some(u) = rr.unsupported {
        eprintln!("partex: ssa rebuild {n}: stopped: {u}");
        return Err(3);
    }
    Ok((rr.edits > 0).then_some(rr.history))
}

/// The SSA build's link (DESIGN 7.17.3, "The link after a rebuild is a
/// watch's link"): a chunk per call of a step that made effects, keyed by
/// the step's id and its place in the step, taken from the last link when
/// its version is the one linked then and its entry is the same
/// (`effects::link_cached`); deflate memoized by content; only the files
/// whose bytes changed written. `PARTEX_LINK_SPLICE=0`: a full link and
/// every file written, each time, as before.
#[derive(Default)]
struct SsaLinker {
    cache: partex_core::effects::LinkCache,
    /// Each chunk's version at the last link, by key.
    linked: std::collections::HashMap<u64, u128>,
    /// Deflate's output by its input's hash, as the last link used it.
    deflated: std::collections::HashMap<u128, Vec<u8>>,
    /// The hash of the bytes last written to each file, by name.
    written: std::collections::BTreeMap<Vec<u8>, u128>,
    /// The highest id of a file the engine had opened at the last link:
    /// a file opened since was made anew by its open (the host's ids
    /// grow).
    opened: Option<u32>,
}

impl SsaLinker {
    /// Link `tex`'s files from its steps' effects and write them: each
    /// file by the name it was opened with (the last open of a name
    /// wins), the terminal's text, the diagnostics. What it cost, for the
    /// report.
    fn link(&mut self, tex: &mut Tex<native::NativeHost, partex_core::ssa::SsaTracker>) -> String {
        use partex_core::host::Host;
        let splice = !std::env::var("PARTEX_LINK_SPLICE").is_ok_and(|v| v == "0");
        let chunks = partex_core::ssa::step_effects(&tex.tracker().rec.borrow());
        if std::env::var("PARTEX_SSA_LINK_TRACE").is_ok_and(|v| v == "1") {
            trace_ssa_link(&chunks);
        }
        let threads = partex_incr::Threads::available();
        let origin = std::time::Instant::now();
        let clock = || u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let mut times = partex_core::effects::LinkTimes::default();
        let versions: std::collections::HashMap<u64, u128> =
            chunks.iter().map(|(k, e)| (*k, e.0.0)).collect();
        let (mut was, mut now) = (
            std::mem::take(&mut self.deflated),
            std::collections::HashMap::new(),
        );
        // (content-keyed: a stream made again alike compresses alike)
        let mut deflate = |level: i32, data: &[u8]| {
            if !splice {
                return crate::zlib::deflate_stream(level, data);
            }
            let key = partex_core::StableHasher::of(&(b"deflate", level, data));
            if let Some(z) = now.get(&key) {
                return Some(Vec::clone(z));
            }
            let z = match was.remove(&key) {
                Some(z) => z,
                None => crate::zlib::deflate_stream(level, data)?,
            };
            now.insert(key, z.clone());
            Some(z)
        };
        let linked = if splice {
            let keyed: Vec<(u64, &[partex_core::effects::Effect])> =
                chunks.iter().map(|(k, e)| (*k, &e.1[..])).collect();
            let touched = |k: u64| versions.get(&k) != self.linked.get(&k);
            partex_core::effects::link_cached_timed(
                &keyed,
                &touched,
                &mut self.cache,
                &threads,
                &mut deflate,
                &clock,
                &mut times,
            )
        } else {
            let slices: Vec<&[partex_core::effects::Effect]> =
                chunks.iter().map(|(_, e)| &e.1[..]).collect();
            partex_core::effects::link(&slices, &threads, &mut deflate)
        };
        drop(deflate);
        self.deflated = now;
        self.linked = versions;
        let l = match linked {
            Ok(l) => l,
            Err(e) => {
                eprintln!("partex: ssa: the link step failed: {e:?}");
                std::process::exit(3);
            }
        };
        if splice && cfg!(debug_assertions) {
            let slices: Vec<&[partex_core::effects::Effect]> =
                chunks.iter().map(|(_, e)| &e.1[..]).collect();
            let full = partex_core::effects::link(&slices, &threads, &mut |level, data| {
                crate::zlib::deflate_stream(level, data)
            });
            assert!(
                full.as_ref()
                    .is_ok_and(|f| f.files == l.files && f.term == l.term),
                "the SSA link taken from the cache is not a full link's"
            );
        }
        let t_link = origin.elapsed();
        let host = tex.host_mut();
        let (files, bytes_out) = self.write(host, &l, splice);
        host.term_write(&l.term);
        for d in &l.diagnostics {
            host.diagnostic(d);
        }
        #[allow(clippy::cast_precision_loss, reason = "a report")]
        let ms = |ns: u64| ns as f64 / 1e6;
        format!(
            "{} of {} chunks resolved; ms: numbering {:.1}, resolve {:.1}, layout {:.1} \
             (object streams {:.1}, cross-reference {:.1}), lengths {:.1}, copy {:.1}; \
             link {:.1}, files written {:.1} ({files} files, {bytes_out} bytes)",
            self.cache.resolved,
            chunks.len(),
            ms(times.numbering),
            ms(times.resolve),
            ms(times.layout),
            ms(times.objstm),
            ms(times.xref),
            ms(times.lengths),
            ms(times.copy),
            t_link.as_secs_f64() * 1e3,
            origin.elapsed().saturating_sub(t_link).as_secs_f64() * 1e3,
        )
    }

    /// Write the linked files: each by the name it was opened with (the
    /// last open of a name wins), unless `splice` and its bytes are the
    /// ones last written and no step opened it since. The files and bytes
    /// written.
    fn write(
        &mut self,
        host: &mut native::NativeHost,
        l: &partex_core::effects::Linked,
        splice: bool,
    ) -> (usize, usize) {
        use partex_core::host::Host;
        let mut last: std::collections::BTreeMap<Vec<u8>, (u32, partex_core::host::FileKind)> =
            std::collections::BTreeMap::new();
        for (id, name, kind) in &l.opened {
            // (the engine's handle: its bytes are the link's to write)
            host.close(*id);
            last.insert(name.clone(), (id.0, *kind));
        }
        // (the files opened since the last link: their opens emptied them)
        let anew: std::collections::BTreeSet<&[u8]> = l
            .opened
            .iter()
            .filter(|(id, ..)| self.opened.is_none_or(|m| id.0 > m))
            .map(|(_, name, _)| &name[..])
            .collect();
        self.opened = l.opened.iter().map(|(id, ..)| id.0).max().max(self.opened);
        let (mut files, mut bytes_out) = (0, 0);
        for (name, (id, kind)) in &last {
            let bytes = l.files.get(id).map_or(&[][..], Vec::as_slice);
            let h = partex_core::StableHasher::of(bytes);
            if splice && !anew.contains(&name[..]) && self.written.get(name) == Some(&h) {
                continue;
            }
            if let Some((w, _)) = host.open_write(name, *kind) {
                host.write(w, bytes);
                host.close(w);
            }
            self.written.insert(name.clone(), h);
            files += 1;
            bytes_out += bytes.len();
        }
        (files, bytes_out)
    }
}

/// `PARTEX_SSA_LINK_TRACE=1`: each chunk's step, the files it opens and
/// closes and the bytes it writes to each.
fn trace_ssa_link(chunks: &[(u64, partex_core::ssa::StepEffects)]) {
    use partex_core::effects::Effect;
    for (i, (k, c)) in chunks.iter().enumerate() {
        let mut w: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
        let mut ev = Vec::new();
        for e in c.1.iter() {
            match e {
                Effect::Write { file, bytes } => *w.entry(file.0).or_default() += bytes.len(),
                Effect::Open { file, name, .. } => {
                    ev.push(format!("open {} {}", file.0, String::from_utf8_lossy(name)));
                }
                Effect::Close(f) => ev.push(format!("close {}", f.0)),
                _ => {}
            }
        }
        if !w.is_empty() || !ev.is_empty() {
            eprintln!(
                "partex: ssa link {i} (step {}): {} writes {w:?}",
                k >> 32,
                ev.join(", ")
            );
        }
    }
}

/// Check mode's report of one build (`PARTEX_SSA_CHECK=1`): the rows not
/// values yet, the parts a hit left uncovered, the stale reads and the
/// writes a hit's record and its body made differently.
fn report_check(r: &partex_core::ssa::SsaReport, rec: &partex_core::ssa::Recorder) {
    let mut v: Vec<_> = r.uncovered.iter().collect();
    v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
    eprintln!(
        "partex: ssa check: {} rows not values yet: {}",
        r.not_values.len(),
        r.not_values.join("; ")
    );
    eprintln!(
        "partex: ssa check: {} hits checked, {} uncovered parts",
        r.checked,
        v.len()
    );
    for (part, (n, first)) in v {
        eprintln!("partex: ssa uncovered {n:7} {part} (first: {first})");
    }
    let stale = &rec.st.stale;
    eprintln!(
        "partex: ssa check: table reads whose version was not the content: {}",
        if stale.is_empty() {
            "none".to_string()
        } else {
            format!("{stale:?}")
        }
    );
    for s in &rec.st.stale_first {
        eprintln!("partex: ssa stale {s}");
    }
    for d in &r.write_diffs {
        eprintln!("partex: ssa write differs {d}");
    }
}

/// The reads a build noted and verified, by family (`PARTEX_SSA=1`).
/// The routines recorded through `Tracker::call_begin`: a build's calls,
/// its (probed) hits and the records made for each.
fn report_routines(build: &str, routines: &[partex_core::ssa::RoutineCount]) {
    let routines: Vec<String> = partex_core::ssa::Func::ALL
        .iter()
        .zip(routines)
        .filter(|(_, c)| c.calls > 0 || c.records > 0)
        .map(|(f, c)| format!("{f} {} (hits {}, records {})", c.calls, c.hits, c.records))
        .collect();
    eprintln!("partex: ssa {build} routines: {}", routines.join(", "));
}

fn report_read_counts(rec: &partex_core::ssa::Recorder) {
    use std::fmt::Write as _;
    let st = &rec.st;
    let mut line = String::new();
    for i in 0..st.noted.len() {
        let (n, v) = (st.noted[i], st.verified[i].get());
        if n + v > 0 {
            let _ = write!(line, " {}={n}/{v}", partex_core::ssa::count_name(i));
        }
    }
    eprintln!("partex: ssa reads noted/verified by family:{line}");
}

fn run_memo(host: native::NativeHost, mut params: Params, command_line: &[u8]) -> i32 {
    // (`PARTEX_MEMO=1`: memoized macro calls; `=stats` also reports them)
    let memo = std::env::var("PARTEX_MEMO").unwrap_or_default();
    params.memo = !memo.is_empty() && memo != "0";
    let mut tex = Tex::new(host, Untracked, params);
    if memo == "check" {
        tex.set_memo_check();
    }
    // (`PARTEX_MEMO_LIMIT=n`: replay only the first n hits, to bisect)
    let limit = std::env::var("PARTEX_MEMO_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok());
    if let Some(n) = limit {
        tex.set_memo_limit(n);
    }
    // (`PARTEX_EFFECTS=1`: outputs as values, linked at the end; the
    // reference path writes them as they are made)
    let effects = std::env::var("PARTEX_EFFECTS").is_ok_and(|v| v == "1");
    tex.set_effects(effects);
    let history = tex.run(command_line);
    if effects {
        deliver_effects(&mut tex);
    }
    if limit.is_some() {
        eprintln!(
            "partex: memo last hit {}",
            String::from_utf8_lossy(&tex.memo_last_hit())
        );
    }
    if memo == "stats" || memo == "check" {
        eprintln!("partex: memo {:?}", tex.memo_stats());
        for (name, [calls, hits, stale, rec, bad], why) in tex.memo_defs(
            std::env::var("PARTEX_MEMO_DEFS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(40),
        ) {
            eprintln!(
                "partex: memo def {calls:9} calls {hits:8} hits {stale:8} stale {rec:7} rec {bad:3} bad {} {why}",
                String::from_utf8_lossy(&name)
            );
        }
        for (name, n) in tex.memo_stale_cells() {
            eprintln!(
                "partex: memo stale {n:9} {}",
                String::from_utf8_lossy(&name)
            );
        }
        for d in tex.memo_read_diffs() {
            eprintln!("partex: memo {}", String::from_utf8_lossy(&d));
        }
        for (old, new) in tex.memo_pending_diffs() {
            eprintln!("partex: memo stored: {}", String::from_utf8_lossy(&old));
            eprintln!("partex: memo fresh:  {}", String::from_utf8_lossy(&new));
        }
        for (name, what) in tex.memo_mismatches() {
            eprintln!(
                "partex: memo check: {} differs in {what}",
                String::from_utf8_lossy(&name)
            );
        }
    }
    history
}
