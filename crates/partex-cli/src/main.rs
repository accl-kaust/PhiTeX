//! `partex` command-line driver: `tex`-compatible options and a native
//! [`Host`](partex_core::Host).

mod bibtex;
mod cache;
mod clock;
mod compat;
mod config;
#[cfg(feature = "deps")]
mod deps;
mod display;
mod dpxfiles;
mod dvithread;
mod editor;
mod eventlog;
mod events;
mod fontindex;
mod heap;
mod inotify;
mod intervals;
mod json;
mod live;
mod lz;
mod machinehost;
mod makeindex;
mod modern;
mod native;
mod origins;
mod outline;
mod render;
mod resident;
mod sanitize;
mod session;
mod snippet;
mod store;
mod synctexfile;
mod term;
#[cfg(feature = "deps")]
mod texprof;
mod timeline;
mod tokendeps;
mod view;
mod warnings;
mod ws;
mod zlib;

#[cfg(all(feature = "jemalloc", not(target_family = "wasm")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

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
/// `latex`, … are pdfTeX under other names (the name picks the format);
/// `xelatex` is `XeTeX`.
fn flavor_of(progname: &str) -> Flavor {
    match progname {
        "pdftex" | "etex" | "pdfetex" | "pdflatex" | "latex" | "pdfcsplain" | "dvilualatex"
        | "mex" | "pdfmex" | "utf8mex" | "amstex" | "eplain" | "texsis" => Flavor::PdfTex,
        "xetex" | "xelatex" | "xelatex-dev" => Flavor::XeTeX,
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
                    "phitex: Bad value ({v}) in environment or texmf.cnf for {name}, keeping {var}."
                );
            } else {
                *var = i32::try_from(v).unwrap_or(i32::MAX);
            }
        }
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: phitex [-engine=tex|pdftex|xetex] [-ini] [-etex] [-interaction=MODE] [-jobname=NAME] [-output-comment=S] [-shell-escape|-shell-restricted|-no-shell-escape] [-watch [-checkpoint-every=N]] [-resident] [-converge] ARGS..."
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
    // (texmfmp.c's `init_shell_escape`: the commands restricted shell
    // escape runs)
    if params.shell_escape && params.restricted_shell {
        params.shell_escape_commands = kpse
            .var_value("shell_escape_commands")
            .map(|v| partex_core::shell::command_list(&v))
            .unwrap_or_default();
    }
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
    "synctex",
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
        "no-pdf" => params.no_pdf = true,
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
    // (each command line its own: `-synctex` as this one says)
    origins::clear_synctex();
    for a in joined_args(args().into_iter().skip(1)) {
        let opt = a.strip_prefix("--").or_else(|| a.strip_prefix('-'));
        match opt {
            _ if !rest.is_empty() => rest.push(a),
            Some("version") => {
                println!("PhiTeX {}", env!("CARGO_PKG_VERSION"));
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
            Some(o) if o.starts_with("synctex=") => origins::set_synctex(&o[8..]),
            Some(o) if o.starts_with("engine=") => {
                engine = match &o[7..] {
                    "tex" => Flavor::Tex,
                    "pdftex" => Flavor::PdfTex,
                    "xetex" => Flavor::XeTeX,
                    _ => usage(),
                };
                if progname == "tex" && engine == Flavor::PdfTex {
                    "pdftex".clone_into(&mut progname);
                }
                if progname == "tex" && engine == Flavor::XeTeX {
                    "xetex".clone_into(&mut progname);
                }
            }
            Some("etex") => params.etex = true,
            Some(o) if set_option(&mut params, o) => {}
            Some("file-line-error") => params.file_line_error_style = true,
            Some("halt-on-error") => params.halt_on_error = true,
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
        eprintln!("phitex: PARTEX_PDFTEX_BUGS: unknown pdfTeX bug `{name}`; known:");
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
        Flavor::XeTeX => "xetex",
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
            eprintln!("phitex: {name} is not supported yet");
            std::process::exit(2);
        }
    }
}

/// The debugging switches: the expansion watchdog
/// (`PARTEX_WATCHDOG=N`), and the SSA's class and soft reads off
/// (`PARTEX_SSA_CLASS_READS=0`, `PARTEX_SSA_SOFT_READS=0`), to compare a
/// build with them and without.
fn debug_switches() {
    if let Some(n) = std::env::var("PARTEX_WATCHDOG")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        partex_core::WATCHDOG.store(n, std::sync::atomic::Ordering::Relaxed);
    }
    if std::env::var("PARTEX_SSA_CLASS_READS").is_ok_and(|v| v == "0") {
        partex_core::ssa::CLASS_READS.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(p) = std::env::var("PARTEX_SSA_WATCH_SLOT")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        partex_core::ssa::WATCH_SLOT.store(p, std::sync::atomic::Ordering::Relaxed);
        if let Some(f) = std::env::var("PARTEX_SSA_WATCH_FROM")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            partex_core::ssa::WATCH_FROM.store(f, std::sync::atomic::Ordering::Relaxed);
        }
        partex_core::ssa::ENTRY_CHECK.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if std::env::var("PARTEX_SSA_ENTRY_CHECK").is_ok_and(|v| v == "1") {
        partex_core::ssa::ENTRY_CHECK.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if std::env::var("PARTEX_SSA_VOBJ").is_ok_and(|v| v == "0") {
        partex_core::ssa::VOBJ.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    if std::env::var("PARTEX_SSA_FONT_REFS").is_ok_and(|v| v == "0") {
        partex_core::ssa::FONT_REFS.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    if std::env::var("PARTEX_SSA_FLOW").is_ok_and(|v| v == "0") {
        partex_core::ssa::FLOW.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    if std::env::var("PARTEX_SSA_LAZY_VERSIONS").is_ok_and(|v| v == "0") {
        partex_core::ssa::LAZY_VERSIONS.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    if std::env::var("PARTEX_SSA_DEAD_SAVES").is_ok_and(|v| v == "0") {
        partex_core::ssa::DEAD_SAVES.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    if std::env::var("PARTEX_SSA_SOFT_PLACE").is_ok_and(|v| v == "0") {
        partex_core::ssa::SOFT_PLACE.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    if std::env::var("PARTEX_SSA_SOFT_READS").is_ok_and(|v| v == "0") {
        partex_core::ssa::SOFT_READS_ON.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

fn main() {
    debug_switches();
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
            eprintln!("phitex: the event log: {e}");
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
            eprintln!("phitex: done {}", line.trim());
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
    serve_watched(s, history, converge, between, observe, &mut Vec::new)
}

/// [`serve_observed`] for a watch: before each pass that settles the
/// job's own files, `edited` names the files saved since (`ch05.tex`);
/// if any, the build is superseded and the pass is the first of the
/// rebuild for them (with the job's files as the last pass left them),
/// so a save is taken up at the next pass, not after the job settled.
fn serve_watched(
    s: &mut session::Session,
    history: Option<i32>,
    converge: bool,
    between: &mut Between,
    observe: &mut dyn FnMut(events::Progress),
    edited: &mut dyn FnMut() -> Vec<String>,
) -> (Vec<String>, Vec<u8>, i32) {
    let mut reports = Vec::new();
    let mut h = history.unwrap_or(0);
    observe(events::Progress::PassStart(1));
    if history.is_none() {
        observe(events::Progress::Phase(events::Phase::Cold));
    }
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
            None => reports.push(String::from("phitex: unchanged")),
        }
        observe(events::Progress::Phase(events::Phase::Writing));
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
                reports.push(format!("phitex: writing the outputs failed: {e}"));
                return (reports, Vec::new(), 3);
            }
        };
        if converge && r.is_some() {
            // (BibTeX and makeindex between passes, as the conventional
            // pipeline runs them)
            let ending = |ext: &[u8]| -> Vec<(Vec<u8>, std::sync::Arc<[u8]>)> {
                s.outputs_ending(ext)
                    .into_iter()
                    .map(|(n, c)| (n, c.into()))
                    .collect()
            };
            let mut tools = bibtex::after_pass(&mut between.bib, &ending(b".aux"));
            tools.extend(makeindex::after_pass(&mut between.idx, &ending(b".idx")));
            for t in &tools {
                observe(events::Progress::Tool(t));
            }
            reports.extend(tools);
        }
        if !converge || r.is_none() || passes == PASSES {
            return (reports, term, h);
        }
        // (the pass's PDF is complete: shown while the next settles)
        observe(events::Progress::Settling(passes));
        // (what the tools wrote is the job's own, not a save)
        let tooled: Vec<std::path::PathBuf> = std::mem::take(&mut between.bib.written)
            .into_iter()
            .chain(std::mem::take(&mut between.idx.written))
            .map(|(p, _)| p)
            .collect();
        let plain = |p: &std::path::Path| p.strip_prefix(".").unwrap_or(p).to_path_buf();
        let saved: Vec<String> = edited()
            .into_iter()
            .filter(|s| {
                !tooled
                    .iter()
                    .any(|t| plain(t) == plain(std::path::Path::new(s)))
            })
            .collect();
        let what = if saved.is_empty() {
            passes += 1;
            observe(events::Progress::PassStart(passes));
            "converged a pass"
        } else {
            observe(events::Progress::Superseded(&saved));
            passes = 1;
            observe(events::Progress::PassStart(1));
            "rebuilt"
        };
        r = s.rebuild().map(|r| (what, r));
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
        key.and_then(cache::get).is_some_and(|saved| {
            observe(events::Progress::Phase(events::Phase::Loading));
            s.load(saved)
        })
    };
    let mut reports = Vec::new();
    if loaded {
        reports.push(format!(
            "phitex: loaded a saved session in {:.1} ms: {} checkpoints",
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
        observe(events::Progress::Phase(events::Phase::Saving));
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
                    "phitex: save: written in {:.1} ms",
                    t1.elapsed().as_secs_f64() * 1e3
                );
            }
            reports.push(format!(
                "phitex: saved the session in {:.1} ms: {} bytes",
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
        "phitex: {what} in {:.1} ms: {} of {} commands run, {} checkpoints{}{}",
        r.elapsed.as_secs_f64() * 1e3,
        r.commands,
        r.total_commands,
        s.checkpoints(),
        r.cut_at
            .map(|c| format!(", converged at command {c}"))
            .unwrap_or_default(),
        if r.history > 1 { " (with errors)" } else { "" }
    ) + &r.why.iter().fold(String::new(), |mut s, w| {
        s.push_str("\nphitex:   ");
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
            eprintln!("phitex: the link step failed: {e:?}");
            std::process::exit(3);
        }
    }
}

/// `PARTEX_SSA_RERUN_CHECK=1`: every window of the cold build run again
/// alone, the last first (DESIGN 4.3 item 1); whether each ended where it
/// did and made what it made.
fn rerun_check(tex: &mut Tex<native::NativeHost, partex_core::ssa::SsaTracker>) -> bool {
    // (every window run again alone, the last first: DESIGN 4.3 item 1)
    let t = std::time::Instant::now();
    let c = partex_core::ssa::rerun_check(tex, true);
    eprintln!(
        "phitex: ssa rerun check: {:.1} ms, steps {}, commands {}, runs dropped {}, \
         ended elsewhere {}, definitions changed {}, stores changed {}, effects changed {}",
        t.elapsed().as_secs_f64() * 1e3,
        c.steps,
        c.commands,
        c.retries,
        c.ended_elsewhere,
        c.defs_changed,
        c.stores_changed,
        c.effects_changed,
    );
    for l in &c.first {
        eprintln!("phitex: ssa rerun check: {l}");
    }
    if c.ended_elsewhere + c.defs_changed + c.stores_changed + c.effects_changed > 0 {
        // (a step's boundary left state outside the families: the
        // harness sees the process fail)
        eprintln!("phitex: ssa rerun check: FAILED");
        return false;
    }
    true
}

/// The commands a window of the SSA build's steps runs at most (DESIGN
/// 4.3 item 1; `PARTEX_SSA_WINDOW`, default 4096; `0`: steps from one
/// clean point to the next).
fn ssa_window() -> u64 {
    std::env::var("PARTEX_SSA_WINDOW")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4096)
}

/// The SSA build's tracker, as the environment sets it up.
fn ssa_tracker() -> partex_core::ssa::SsaTracker {
    use partex_core::ssa::{Recorder, SsaTracker};
    // (records for the steps and the typesetting calls only, DESIGN 4.3
    // item 2; `=0`: every routine's, and the steps' own reads)
    let mut tracker = SsaTracker::new(Recorder::new());
    tracker.set_lean(!std::env::var("PARTEX_SSA_LEAN").is_ok_and(|v| v == "0"));
    // (the names a run makes placed by name, DESIGN 3.9's allocators)
    tracker.set_names_by_name(std::env::var("PARTEX_SSA_NAMES").is_ok_and(|v| v == "1"));
    tracker.cancel.set(cancel_after(0));
    // (a trip that ends fatally keeps the last complete trip's streams,
    // as an editor wants, not pdfTeX's cut `.aux`: DESIGN 3.7, "A trip
    // that ended fatally")
    tracker
        .keep_complete
        .set(std::env::var("PARTEX_SSA_KEEP_COMPLETE").is_ok_and(|v| v == "1"));
    // (the steps' reads and writes timed, for the graph `write_dag` prints)
    tracker.set_timed(std::env::var_os("PARTEX_SSA_DAG").is_some());
    // (a rebuild runs this many commands at most: past them it stops, as
    // one it cannot make, its trace printed)
    if let Some(b) = std::env::var("PARTEX_SSA_REBUILD_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        tracker.budget.set(b);
    }
    tracker
}

/// The side files asked for: glyph origins (`PARTEX_ORIGINS=1`) and
/// display lists (`PARTEX_DISPLAY=1`), turned on before the cold build.
fn side_files_setup<T: partex_core::track::Tracker>(tex: &mut Tex<native::NativeHost, T>) {
    origins::setup(tex);
    display::setup(tex);
}

/// The side files written after a build or rebuild.
fn side_files_write<T: partex_core::track::Tracker>(tex: &mut Tex<native::NativeHost, T>) {
    origins::write(tex);
    display::write(tex);
}

/// `PARTEX_SSA=1`: the build on the dynamic-SSA runtime (DESIGN.md
/// §7.17, `partex_core::ssa`), with `PARTEX_SSA_CHECK=1` check mode.
/// `PARTEX_SSA_REBUILD=<shell command>` runs the command after the cold
/// build (an edit) and builds again with the same records;
/// `PARTEX_SSA_TRACE=<file>` writes the last build's trace there;
/// `PARTEX_SSA_LEAN=0` records every routine, and each step's own reads.
#[allow(clippy::too_many_lines)]
fn run_ssa(mut host: native::NativeHost, params: Params, command_line: &[u8]) -> i32 {
    host.commands = Some(native::Commands::default());
    let check = std::env::var("PARTEX_SSA_CHECK").is_ok_and(|v| v == "1");
    let rebuild = std::env::var("PARTEX_SSA_REBUILD").ok();
    let apply = std::env::var("PARTEX_SSA_APPLY").is_ok_and(|v| v == "1");
    let trips = ssa_trips();
    // (BibTeX's and makeindex's last runs, across the builds' trips)
    let mut between = Between::default();
    let mut tex = Tex::new(host, ssa_tracker(), params);
    tex.set_window(ssa_window());
    side_files_setup(&mut tex);
    if rebuild.is_some() {
        // (with rebuilds, `PARTEX_SSA_CANCEL_AFTER` cancels each rebuild,
        // not the cold build: `rebuild_ssa`)
        tex.tracker().cancel.set(None);
    }
    let t0 = std::time::Instant::now();
    let r = partex_core::ssa::run_applying(&mut tex, command_line, check, 0, apply);
    if r.cancelled {
        return cancelled();
    }
    if check {
        eprintln!("phitex: ssa lost writes {}", tex.lost_writes());
    }
    // (the cold build converges as latexmk would from the files on disk:
    // its trips after the first, DESIGN 3.7)
    // (with one trip a build, the tools still run after it, as after a
    // pass: `settle` stops at its bound)
    let native = ssa_native();
    let settled = (trips > 1 || native.is_some()).then(|| {
        let ns = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let mut tools = ssa_tools(&mut between);
        let mut t = partex_core::ssa::Trips {
            max: trips,
            tools: &mut tools,
            native: native.as_ref(),
            clock: Some(clock_ns),
        };
        let (trace, apply) = rebuild_switches();
        partex_core::ssa::settle(&mut tex, trace, apply, &mut t, r.commands, ns)
    });
    let millis = t0.elapsed().as_secs_f64() * 1e3;
    report_entry_check(&tex, 0);
    if std::env::var("PARTEX_SSA_RERUN_CHECK").is_ok_and(|v| v == "1") && !rerun_check(&mut tex) {
        return 3;
    }
    // (the collector freed much of the heap while the build ran: sorted
    // and given back now, not at the first keystrokes' allocations)
    heap::trim();
    if std::env::var_os("PARTEX_SSA_MEM").is_some() {
        eprintln!("phitex: ssa build 0: settled: {}", machinehost::rss());
    }
    let mut linker = SsaLinker::default();
    let lr = linker.link(&mut tex);
    linker.write_produced(&mut tex);
    if std::env::var_os("PARTEX_SSA_MEM").is_some() {
        eprintln!("phitex: ssa build 0: linked: {}", machinehost::rss());
    }
    dump_streams(&tex, 0);
    side_files_write(&mut tex);
    ready_for_rebuilds(&tex, rebuild.is_some());
    eprintln!(
        "phitex: ssa build 0: link {:.1} ms: {}; files written {:.1} ms",
        lr.link_ms, lr.how, lr.write_ms
    );
    let mut history = r.history;
    let mut commands = r.commands;
    if let Some(s) = &settled {
        for l in &s.log {
            eprintln!("phitex: ssa build 0: {l}");
        }
        report_trips("build 0", s);
        if std::env::var_os("PARTEX_SSA_MEM").is_some() {
            eprintln!(
                "phitex: ssa build 0: memory: {}; {}",
                tex.tracker().mem_report(),
                machinehost::rss()
            );
        }
        if s.trips > 1 {
            history = s.history;
            commands += s.commands;
        }
        if let Some(u) = s.unsupported {
            eprintln!("phitex: ssa build 0: stopped: {u}");
            return 3;
        }
    }
    {
        let rec = tex.tracker().rec.borrow();
        report_read_counts(&rec);
        let s = rec.rt.stats;
        eprintln!(
            "phitex: ssa build 0: {:.1} ms, calls {} (hits {}, misses {}, fresh {}), \
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
            commands,
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
    write_view(&tex, 0);
    // (one rebuild after each line's command)
    let lines: Vec<&str> = rebuild.as_deref().unwrap_or_default().lines().collect();
    for (n, cmd) in lines.iter().enumerate() {
        match rebuild_ssa(
            &mut tex,
            &mut linker,
            &mut between,
            native.as_ref(),
            trips,
            n + 1,
            Some(cmd),
        ) {
            Ok(Some(h)) => history = h,
            Ok(None) => {}
            Err(code) => return code,
        }
    }
    if std::env::var_os("PARTEX_SSA_MEM").is_some() {
        eprintln!("phitex: ssa: after the rebuilds: {}", machinehost::rss());
    }
    // (a rebuild that stopped left work: it goes on, unstopped, and the
    // files are linked, DESIGN 3.7, "A rebuild stopped")
    if partex_core::ssa::pending(&tex) > 0 {
        tex.tracker().cancel.set(None);
        match rebuild_ssa(
            &mut tex,
            &mut linker,
            &mut between,
            native.as_ref(),
            trips,
            lines.len() + 1,
            None,
        ) {
            Ok(Some(h)) => history = h,
            Ok(None) => {}
            Err(code) => return code,
        }
    }
    if let Some(path) = std::env::var_os("PARTEX_SSA_TRACE") {
        let _ = std::fs::write(path, tex.tracker().rec.borrow().rt.trace().to_text());
    }
    // (the process ends after this: the records go with it rather than be
    // freed one by one, which was 2% of the thesis's cold build; the
    // engine and its host drop as usual, their files closed)
    std::mem::forget(tex.take_recorder());
    history
}

/// Whether rebuild `n` is traced (`PARTEX_SSA_REBUILD_TRACE`): `1` traces
/// every rebuild; a list of numbers and ranges (`67,70-72`; `1-1` for the
/// first alone) traces those, so a long sequence is timed untraced and
/// traced where it matters, in one process.
fn rebuild_traced(n: usize) -> bool {
    let Ok(v) = std::env::var("PARTEX_SSA_REBUILD_TRACE") else {
        return false;
    };
    if v == "1" {
        return true;
    }
    let num = |s: &str| s.trim().parse::<usize>().ok();
    v.split(',').any(|p| match p.split_once('-') {
        Some((a, b)) => num(a).is_some_and(|a| a <= n) && num(b).is_some_and(|b| n <= b),
        None => num(p) == Some(n),
    })
}

/// Rebuild `n` of an SSA build (DESIGN 7.17.3): run the edit `cmd`, then
/// rebuild the same engine in place and link its files; the exit code it
/// sets, if its edits changed the job's (`Err`: it failed or stopped
/// short). With no `cmd`, the work a stopped rebuild left, unstopped. A
/// rebuild that stops (`PARTEX_SSA_REBUILD_MS`, `PARTEX_SSA_CANCEL_AFTER`,
/// each counted from the rebuild's start) keeps its work and links
/// nothing: the next one goes on with it.
#[allow(clippy::too_many_lines)]
fn rebuild_ssa(
    tex: &mut Tex<native::NativeHost, partex_core::ssa::SsaTracker>,
    linker: &mut SsaLinker,
    between: &mut Between,
    native: Option<&partex_core::ssa::NativeTools>,
    trips: usize,
    n: usize,
    cmd: Option<&str>,
) -> Result<Option<i32>, i32> {
    if let Some(cmd) = cmd {
        let ok = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("phitex: ssa: the rebuild command failed");
            return Err(3);
        }
        tex.tracker().cancel.set(cancel_after(n));
        tex.tracker().deadline.set(rebuild_deadline());
    } else {
        tex.tracker().deadline.set(None);
    }
    // (the same engine, rebuilt in place: DESIGN 7.17.3)
    let before = tex.tracker().rec.borrow().rt.stats;
    let routines = tex.tracker().rec.borrow().st.routines;
    let t0 = std::time::Instant::now();
    // (the rebuilds traced: `PARTEX_SSA_REBUILD_TRACE`, rebuild_traced)
    let trace = rebuild_traced(n);
    let (_, apply) = rebuild_switches();
    // (in trips until the loads read what the same trip stored, DESIGN
    // 3.7; `PARTEX_SSA_TRIPS=1`: one, a plain pass)
    let mut tools = ssa_tools(between);
    // (`PARTEX_SSA_KEY_TRIPS=k`: each edit's rebuild runs k trips at most,
    // as an editor's keystroke would; with `PARTEX_SSA_IDLE_SETTLE=1` a
    // settle of `trips` trips follows it, as the editor's idle one)
    let key_trips = std::env::var("PARTEX_SSA_KEY_TRIPS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&k| k >= 1);
    let idle = std::env::var("PARTEX_SSA_IDLE_SETTLE").is_ok_and(|v| v == "1");
    let mut t = partex_core::ssa::Trips {
        max: key_trips.unwrap_or(trips),
        tools: &mut tools,
        native,
        clock: Some(clock_ns),
    };
    let rr = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        partex_core::ssa::rebuild_trips(tex, trace, apply, &mut t)
    })) {
        Ok(rr) => rr,
        Err(e) => {
            for l in partex_core::ssa::rebuild_log(tex) {
                eprintln!("phitex: ssa rebuild {n}: {l}");
            }
            std::panic::resume_unwind(e);
        }
    };
    if idle && rr.unsupported.is_none() && rr.stopped.is_none() {
        // (the keystroke's trips first, linked, its files written as an
        // editor's would be; then the idle settle's)
        report_trips(&format!("rebuild {n} (keystroke)"), &rr);
        let lr = linker.link(tex);
        linker.write_produced(tex);
        side_files_write(tex);
        eprintln!("phitex: ssa rebuild {n} (keystroke): link: {}", lr.how);
        let mut t = partex_core::ssa::Trips {
            max: trips,
            tools: &mut tools,
            native,
            clock: Some(clock_ns),
        };
        let s = partex_core::ssa::settle(tex, trace, apply, &mut t, 0, 0);
        for l in &s.log {
            eprintln!("phitex: ssa rebuild {n} idle settle: {l}");
        }
        report_trips(&format!("rebuild {n} idle settle"), &s);
    }
    for l in &rr.log {
        eprintln!("phitex: ssa rebuild {n}: {l}");
    }
    let millis = t0.elapsed().as_secs_f64() * 1e3;
    if rr.resumed {
        eprintln!("phitex: ssa rebuild {n}: continued the work a stopped rebuild left");
    }
    report_entry_check(tex, n);
    if let (Some(why), None) = (rr.stopped, rr.unsupported) {
        // (the link waits until the work is done)
        eprintln!(
            "phitex: ssa rebuild {n}: stopped ({why}), {} steps pending; {millis:.1} ms, \
             steps run {}, commands {}",
            rr.pending, rr.steps_run, rr.commands
        );
        return Ok(None);
    }
    let lr = linker.link(tex);
    linker.write_produced(tex);
    dump_streams(tex, n);
    side_files_write(tex);
    let link_ms = lr.link_ms;
    eprintln!("phitex: ssa rebuild {n}: link: {}", lr.how);
    let s = tex.tracker().rec.borrow().rt.stats;
    report_cold(n, &rr);
    eprintln!(
        "phitex: ssa rebuild {n}: {:.1} ms (the rebuild {millis:.1} ms, the link {link_ms:.1} ms), \
         the edited page ready {:.2} ms after the rebuild, files written {:.1} ms, \
         edits {}, seeds {} \
         (loads of a changed φ {}, of a changed store {}, queries answered anew {}; data edited in it {}), \
         steps run {} (new {}, definitions the same {}, runs dropped {}, passed over {}), calls {} (fresh {}, \
         hits applied {} for {} commands), \
         definitions changed {}, readers marked {} (kept {}), reads checked {}, positioned {}, \
         restored {}, from the format {}, commands {}",
        millis + link_ms + lr.write_ms,
        lr.ready_ms,
        lr.write_ms,
        rr.edits,
        rr.seeds,
        rr.phi,
        rr.store_readers,
        rr.queries,
        rr.data_edits,
        rr.steps_run,
        rr.new_steps,
        rr.steps_same,
        rr.retries,
        rr.removed,
        s.hits + s.misses - before.hits - before.misses,
        s.fresh - before.fresh,
        rr.applied,
        rr.skipped,
        rr.defs_changed,
        rr.readers_marked,
        rr.readers_kept,
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
    if trips > 1 {
        report_trips(&format!("rebuild {n}"), &rr);
    }
    if let Some(u) = rr.unsupported {
        eprintln!("phitex: ssa rebuild {n}: stopped: {u}");
        return Err(3);
    }
    write_view(tex, n);
    Ok((rr.edits > 0 || rr.resumed).then_some(rr.history))
}

/// `PARTEX_SSA_ENTRY_CHECK=1`: the reads that did not see what the
/// definitions reaching their step made (`ssa::ENTRY_CHECK`).
fn report_entry_check(tex: &Tex<native::NativeHost, partex_core::ssa::SsaTracker>, n: usize) {
    if !partex_core::ssa::ENTRY_CHECK.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let (count, first) = partex_core::ssa::entry_check_report(tex);
    eprintln!(
        "phitex: ssa build {n}: entry check: {count} reads not as their step's definitions say"
    );
    for l in first {
        eprintln!("phitex: ssa build {n}: entry check: {l}");
    }
}

/// `PARTEX_SSA_DAG=<file>`: the build's steps as a dependency graph
/// (`partex_core::ssa::dag`: each step's commands, the steps whose
/// definitions it read, the versions of its definitions), written to
/// `<file>` after the cold build and to `<file>.N` after rebuild `N`, for
/// `scripts/ssa-parallel.py`. Unset, nothing is made.
fn write_dag(tex: &Tex<native::NativeHost, partex_core::ssa::SsaTracker>, build: usize) {
    let Some(mut path) = std::env::var_os("PARTEX_SSA_DAG") else {
        return;
    };
    if build > 0 {
        path.push(format!(".{build}"));
    }
    let text = partex_core::ssa::dag(tex);
    let shown = std::path::Path::new(&path).display().to_string();
    match std::fs::write(&path, &text) {
        Ok(()) => eprintln!("phitex: ssa dag {build}: {} bytes ({shown})", text.len()),
        Err(e) => eprintln!("phitex: ssa dag {build}: {shown}: {e}"),
    }
}

/// `PARTEX_SSA_TRIPS`: an SSA build's trips at most (DESIGN 3.7, "Trips,
/// as built"), 5 by default, latexmk's bound; 1 is one trip per build,
/// a rebuild matching one plain pass, with no outside tool run.
fn ssa_trips() -> usize {
    std::env::var("PARTEX_SSA_TRIPS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(5)
}

/// A rebuild's switches: its trace (`PARTEX_SSA_REBUILD_TRACE=1`), and
/// whether hits are applied in the steps run (DESIGN 7.17.3: on unless
/// `PARTEX_SSA_APPLY=0`).
fn rebuild_switches() -> (bool, bool) {
    let trace = std::env::var("PARTEX_SSA_REBUILD_TRACE").is_ok_and(|v| v == "1");
    let apply = !matches!(std::env::var("PARTEX_SSA_APPLY").as_deref(), Ok("0"));
    (trace, apply)
}

/// `PARTEX_SSA_REBUILD_MS`: a rebuild stops past this many milliseconds
/// from now, as one past its budget of commands does.
fn rebuild_deadline() -> Option<partex_core::ssa::Deadline> {
    let ms = std::env::var("PARTEX_SSA_REBUILD_MS")
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    let clock: fn() -> u64 = clock_ns;
    Some((
        clock,
        clock_ns().saturating_add(ms.saturating_mul(1_000_000)),
    ))
}

/// A cold build cancelled ([`cancel_after`]): its engine is in the
/// middle of the job, so the run ends there.
fn cancelled() -> i32 {
    eprintln!("phitex: ssa build 0: cancelled at a step boundary");
    3
}

/// `PARTEX_SSA_STREAMS=<dir>`: after build `n`'s link, each written
/// stream as the stores hold it (what a load of it is served) into
/// `<dir>/<n>/`, to compare with the files the link wrote.
fn dump_streams(tex: &Tex<native::NativeHost, partex_core::ssa::SsaTracker>, n: usize) {
    let Ok(dir) = std::env::var("PARTEX_SSA_STREAMS") else {
        return;
    };
    let dir = std::path::Path::new(&dir).join(n.to_string());
    let _ = std::fs::create_dir_all(&dir);
    for (name, bytes) in partex_core::ssa::stream_values(tex) {
        let base = name.rsplit(|&b| b == b'/').next().unwrap_or(&name);
        let file = dir.join(String::from_utf8_lossy(base).as_ref());
        match bytes {
            Some(b) => {
                let _ = std::fs::write(file, b);
            }
            None => {
                let _ = std::fs::write(file.with_extension("none"), b"");
            }
        }
    }
}

/// `PARTEX_SSA_CANCEL_AFTER=N`: [`partex_core::ssa::SsaTracker::cancel`]
/// says to stop from its `N`th question on (the host's signal, as a test
/// stands it in). A list (`N1,N2,...`) gives rebuild `k` (from 1) the
/// `k`th, `0` or none past its end being no cancel; the cold build
/// (`build` 0) is cancelled only by a single `N`.
fn cancel_after(build: usize) -> Option<partex_core::ssa::Cancel> {
    static LEFT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let v = std::env::var("PARTEX_SSA_CANCEL_AFTER").ok()?;
    let n = if v.contains(',') {
        let at = build.checked_sub(1)?;
        v.split(',').nth(at)?.trim().parse::<u64>().ok()?
    } else {
        v.trim().parse::<u64>().ok()?
    };
    if n == 0 {
        return None;
    }
    LEFT.store(n, std::sync::atomic::Ordering::Relaxed);
    Some(|| {
        // (the count left before this question: 1 is the `N`th)
        LEFT.try_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |v| Some(v.saturating_sub(1)),
        )
        .unwrap_or(0)
            <= 1
    })
}

/// A rebuild's cascades that went cold (`RebuildReport::cold`).
fn report_cold(n: usize, rr: &partex_core::ssa::RebuildReport) {
    if rr.cold > 0 {
        eprintln!(
            "phitex: ssa rebuild {n}: cold after {} cascades (the old steps after each retired)",
            rr.cold
        );
    }
}

/// Nanoseconds since the epoch: the trips' clock.
fn clock_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// The build's own BibTeX and makeindex, nodes of its program (DESIGN
/// 3.7, "Outside tools are nodes"), with their `texmf.cnf` settings;
/// `None` with `PARTEX_SSA_TOOLS=0` (none run, as a host without them) or
/// `PARTEX_SSA_TOOLS=outside` (run as outside tools between the trips,
/// [`ssa_tools`]). Made once, for the cold build and every rebuild after
/// it: its kpathsea instance reads TeX Live's `ls-R` databases, 25-30 ms
/// that each keystroke paid when every rebuild made it again.
fn ssa_native() -> Option<partex_core::ssa::NativeTools> {
    if std::env::var("PARTEX_SSA_TOOLS").is_ok_and(|v| v == "0" || v == "outside") {
        return None;
    }
    let mut kpse = kpse_instance("bibtex", "");
    Some(partex_core::ssa::NativeTools {
        bibtex: Some(bibtex::options(&mut kpse, false, 2)),
        makeindex: Some(makeindex::version()),
        // (`PARTEX_SSA_TOOL_CALLS=0`: each run of a tool's node is the
        // whole program, the reference)
        calls: std::env::var("PARTEX_SSA_TOOL_CALLS").map_or(true, |v| v != "0"),
    })
}

/// The outside tools an SSA build runs between its trips with
/// `PARTEX_SSA_TOOLS=outside` (DESIGN 3.7, "Trips, as built", the
/// compatibility path): BibTeX on each stored `.aux` stream, makeindex on
/// each stored `.idx` stream, each read from the build's stores (the
/// files are being rewritten) and run unless what its last run read
/// reads the same, writing its files where the conventional tool does.
#[allow(clippy::type_complexity)]
fn ssa_tools(
    between: &mut Between,
) -> impl FnMut(&mut native::NativeHost, &[(Vec<u8>, std::sync::Arc<[u8]>)]) -> (bool, Vec<String>) + '_
{
    let outside = std::env::var("PARTEX_SSA_TOOLS").is_ok_and(|v| v == "outside");
    let mem = std::env::var_os("PARTEX_SSA_MEM").is_some();
    move |host, streams| {
        if mem {
            eprintln!("phitex: ssa: a trip's end: {}", machinehost::rss());
        }
        // (each stream by the path its file has, in the output directory)
        let ending = |ext: &[u8]| -> Vec<(Vec<u8>, std::sync::Arc<[u8]>)> {
            streams
                .iter()
                .filter(|(name, _)| name.ends_with(ext))
                .map(|(name, c)| {
                    (
                        host.in_output_dir(name).unwrap_or_else(|| name.clone()),
                        c.clone(),
                    )
                })
                .collect()
        };
        if !outside {
            return (false, Vec::new());
        }
        let mut lines = bibtex::after_pass(&mut between.bib, &ending(b".aux"));
        lines.extend(makeindex::after_pass(&mut between.idx, &ending(b".idx")));
        (!lines.is_empty(), lines)
    }
}

/// A build's trips, as the SSA report gives them: each trip's steps,
/// commands and time, whether the build converged, and the outside
/// tools' runs.
#[allow(clippy::cast_precision_loss, reason = "a report")]
fn report_trips(what: &str, r: &partex_core::ssa::RebuildReport) {
    let each: Vec<String> = (0..r.trips)
        .map(|k| {
            format!(
                "trip {}: {} steps, {} commands, {:.1} ms",
                k + 1,
                r.trip_steps.get(k).copied().unwrap_or(0),
                r.trip_commands.get(k).copied().unwrap_or(0),
                r.trip_ns.get(k).copied().unwrap_or(0) as f64 / 1e6
            )
        })
        .collect();
    let state = if r.settled {
        String::from("settled")
    } else if r.unsupported.is_some() || r.stopped.is_some() {
        String::from("stopped")
    } else {
        let names: Vec<String> = r
            .unsettled
            .iter()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect();
        format!(
            "not settled: the loads of {} read what the trip before stored",
            names.join(", ")
        )
    };
    eprintln!(
        "phitex: ssa {what}: trips {} ({state}); {}",
        r.trips,
        each.join("; ")
    );
    for t in &r.tools {
        eprintln!("phitex: ssa {what}: {t}");
    }
}

/// The SSA build's link (DESIGN 3.8; 4.3, item 4: the link costs the
/// changed chunks): the steps whose chunks changed since the last link
/// (`ssa::take_step_changes`) are spliced into the last link's layout
/// (`effects::Splice`: an offset tree per file, object streams and the
/// cross-reference section rendered again only when a change reaches
/// them); deflate memoized by content; each file written from its first
/// changed byte when the file is as last written, else in full.
/// `PARTEX_LINK_SPLICE=0`: a full link of every step's chunks and every
/// file written, each time, as before. A debug build (or
/// `PARTEX_LINK_CHECK=1`) checks each link against a full one.
/// `PARTEX_SSA_VIEW=<file>`: the build as a program (DESIGN 4.3 item 7,
/// `partex_core::ssa::view`), a value per window, written to `<file>`
/// after the cold build and to `<file>.N` after rebuild `N`, checked
/// (`Program::check`); with `PARTEX_SSA_VIEW_STEP=<id>`, step `id`'s calls
/// with their reads and writes too (`partex_core::ssa::step_trace`), to
/// that file's name with `.step<id>` after it. Unset, nothing is made.
/// After the cold build's link: a rebuild compresses its page and the
/// cross-reference stream again, the same up to the edit's place, so
/// deflate goes on from where it was there (`zlib::deflate_stream`;
/// `PARTEX_DEFLATE_RESUME=0`: from the start); with `rebuilds` to come,
/// what the first would decode is decoded now, the output written, not
/// at the first keystroke.
fn ready_for_rebuilds(tex: &Tex<native::NativeHost, partex_core::ssa::SsaTracker>, rebuilds: bool) {
    zlib::resume_streams(!std::env::var("PARTEX_DEFLATE_RESUME").is_ok_and(|v| v == "0"));
    if rebuilds {
        partex_core::ssa::prepare_rebuilds(tex);
    }
}

fn write_view(tex: &Tex<native::NativeHost, partex_core::ssa::SsaTracker>, build: usize) {
    write_dag(tex, build);
    let Some(base) = std::env::var_os("PARTEX_SSA_VIEW") else {
        return;
    };
    let mut path = base;
    if build > 0 {
        path.push(format!(".{build}"));
    }
    let t0 = std::time::Instant::now();
    let prog = partex_core::ssa::view(tex);
    let text = prog.to_text();
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    let checked = match prog.check() {
        Ok(()) => "checked".to_string(),
        Err(e) => format!("NOT SSA: {e}"),
    };
    let shown = std::path::Path::new(&path).display().to_string();
    if let Err(e) = std::fs::write(&path, &text) {
        eprintln!("phitex: ssa view {build}: {shown}: {e}");
        return;
    }
    eprintln!(
        "phitex: ssa view {build}: {} values, {} bytes, {ms:.1} ms, {checked} ({shown})",
        prog.values.len(),
        text.len(),
    );
    let Ok(step) = std::env::var("PARTEX_SSA_VIEW_STEP") else {
        return;
    };
    let Some(trace) = step
        .parse()
        .ok()
        .and_then(|id| partex_core::ssa::step_trace(tex, id))
    else {
        eprintln!("phitex: ssa view {build}: no live step {step}");
        return;
    };
    path.push(format!(".step{step}"));
    if let Err(e) = std::fs::write(&path, trace) {
        eprintln!(
            "phitex: ssa view {build}: {}: {e}",
            std::path::Path::new(&path).display()
        );
    }
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
    splice: partex_core::effects::Splice,
    /// The fold's count of renumberings at the last link.
    renumbered: u32,
    /// Deflate's output by its input's hash, with the link that last used
    /// it.
    deflated: std::collections::HashMap<u128, (Vec<u8>, u64)>,
    links: u64,
    /// What was last written to each file, by name: the engine's id of it,
    /// and the file's length and modification time after the write.
    written: std::collections::BTreeMap<Vec<u8>, (u32, u64, Option<std::time::SystemTime>)>,
    /// The host's count of files opened at the last link: a file opened
    /// since was made anew by its open (`NativeHost::opened_after`).
    opened: u64,
    /// The streams producers outside the steps defined, as last written
    /// (`partex_core::ssa::define_stream`).
    produced: std::collections::BTreeMap<Vec<u8>, u128>,
    /// The full link's chunks as their numbers were resolved, by key
    /// (`effects::link_cached`), and the version of each chunk then: a
    /// keystroke resolves the chunks that changed, not a copy of every
    /// effect of the job.
    resolved: partex_core::effects::LinkCache,
    resolved_versions: std::collections::HashMap<u64, u128>,
    /// With virtual object numbers, each step's entry and signature
    /// (`effects::Resolver`): the spliced link resolves the steps that
    /// changed while the job's numbering holds.
    virt: partex_core::effects::Resolver,
    /// A stream's length in one chunk and its stream in another: the
    /// resolver cannot cut there, so every link is a full one.
    virt_dead: bool,
    /// The last spliced link's times in ns: every step's chunks gathered
    /// (a full resolution), and the resolution with it.
    virt_ns: (u64, u64),
    /// Each file written into place whole, through a file beside it
    /// renamed over it (`phitex watch --ssa`: a reader, the viewer, never
    /// sees half a PDF), not patched from its first changed byte.
    atomic: bool,
    /// The files the links wrote, by the name each was written under, with
    /// its length; the streams `write_produced` wrote too.
    outputs: std::collections::BTreeMap<Vec<u8>, usize>,
    /// The pages of the last link, by `\count0`.
    pages: Vec<i32>,
}

/// Write `bytes` to the file at `path` through a file beside it renamed
/// over it: a reader finds the old file or the new, whole.
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".phitex-new");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// What a link cost, for the reports.
struct LinkReport {
    how: String,
    /// The link (without the files' writes), the time until the changed
    /// chunks were placed, and the writes.
    link_ms: f64,
    ready_ms: f64,
    write_ms: f64,
}

/// How each file was written: files written whole, files written from a
/// byte on, and the bytes.
#[derive(Default)]
struct Written {
    whole: usize,
    patched: usize,
    bytes: u64,
}

impl SsaLinker {
    /// Write the streams producers outside the steps defined whose bytes
    /// changed since they were last written (DESIGN 3.7, "Files are a
    /// view": the link writes every file the build holds), and the
    /// streams a trip that ended fatally withheld, over what the steps'
    /// effects made of them (`partex_core::ssa::withheld_streams`); a job
    /// that ended fatally leaves no PDF (pdfTeX's `remove_pdffile`,
    /// `Tex::fatal_pdf`).
    fn write_produced(&mut self, tex: &mut Tex<native::NativeHost, partex_core::ssa::SsaTracker>) {
        use partex_core::host::Host;
        if let Some(pdf) = tex.fatal_pdf() {
            tex.host_mut().remove_output(&pdf);
        }
        for (name, bytes) in partex_core::ssa::withheld_streams(tex) {
            // (where the job writes it, as `remove_files` finds it)
            let host = tex.host_mut();
            let n = host.in_output_dir(&name).unwrap_or(name);
            let p = native::path(&n);
            let _ = match &bytes {
                Some(b) if self.atomic => write_atomic(&p, b),
                Some(b) => std::fs::write(&p, b),
                None => std::fs::remove_file(&p),
            };
            host.note_written(&n);
        }
        for (name, bytes) in partex_core::ssa::produced_streams(tex) {
            let Some(bytes) = bytes else { continue };
            let v = partex_core::StableHasher::of(&bytes[..]);
            if self.produced.get(&name) == Some(&v) {
                continue;
            }
            let host = tex.host_mut();
            if self.atomic {
                let n = host.in_output_dir(&name).unwrap_or_else(|| name.clone());
                if write_atomic(&native::path(&n), &bytes).is_ok() {
                    host.note_written(&n);
                    self.outputs.insert(n, bytes.len());
                }
            } else if let Some((w, written)) =
                host.open_write(&name, partex_core::host::FileKind::Other)
            {
                host.write(w, &bytes);
                host.close(w);
                host.note_written(&written);
                self.outputs.insert(written, bytes.len());
            }
            self.produced.insert(name, v);
        }
    }

    /// Link `tex`'s files from its steps' effects and write them: each
    /// file by the name it was opened with (the last open of a name
    /// wins), the terminal's text, the diagnostics. What it cost, for the
    /// report.
    fn link(
        &mut self,
        tex: &mut Tex<native::NativeHost, partex_core::ssa::SsaTracker>,
    ) -> LinkReport {
        use partex_core::host::Host;
        let origin = std::time::Instant::now();
        let clock = || u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX);
        #[allow(clippy::cast_precision_loss, reason = "a report")]
        let ms = |ns: u64| ns as f64 / 1e6;
        let splice = !std::env::var("PARTEX_LINK_SPLICE").is_ok_and(|v| v == "0");
        let check =
            cfg!(debug_assertions) || std::env::var("PARTEX_LINK_CHECK").is_ok_and(|v| v == "1");
        let trace = std::env::var("PARTEX_SSA_LINK_TRACE").is_ok_and(|v| v == "1");
        self.links += 1;
        let changes = partex_core::ssa::take_step_changes(&mut tex.tracker().rec.borrow_mut());
        if !splice || trace {
            let rec = tex.tracker().rec.borrow();
            let chunks = partex_core::ssa::step_effects(&rec);
            if trace {
                trace_ssa_link(&chunks);
            }
            if !splice {
                drop(rec);
                return self.link_full(tex, &origin);
            }
        }
        // (virtual object numbers: resolved per step changed, while the
        // job's numbering holds, `effects::Resolver`; a stream's length in
        // one chunk and its stream in another: a full link, every time)
        let virt = tex.virtual_objects();
        if virt && self.virt_dead {
            return self.link_full(tex, &origin);
        }
        let n_changes = changes.len();
        self.virt_ns = (0, 0);
        let (linked, misses) = self.spliced(tex, changes, virt, &clock);
        let out = match linked {
            Ok(Some(out)) => out,
            Ok(None) => {
                self.virt_dead |= virt;
                return self.link_full(tex, &origin);
            }
            // (a byte count's digits changed: the full link renders its
            // text again, and the next link resolves in full)
            Err(_) if virt => return self.link_full(tex, &origin),
            Err(e) => {
                eprintln!("phitex: ssa: the link step failed: {e:?}");
                std::process::exit(3);
            }
        };
        let t_link = clock();
        if check {
            self.check(tex, &out, virt);
        }
        let t_check = clock();
        let removed = partex_core::ssa::removed_files(&tex.tracker().rec.borrow());
        let host = tex.host_mut();
        let w = self.write_spliced(host, &out);
        remove_files(host, &removed);
        host.term_write(&out.term);
        self.splice.each_diagnostic(&mut |d| host.diagnostic(d));
        self.pages = self.splice.pages();
        let t_end = clock();
        let st = self.splice.stats;
        let checked = check.then(|| ms(t_check - t_link));
        LinkReport {
            how: splice_report(
                n_changes,
                &st,
                misses,
                (ms(t_link), checked, ms(t_end - t_check)),
                &w,
            ) + &if virt {
                format!(
                    "; virtual numbers: {} steps laid out ({}), {} chunks resolved again, \
                     ms {:.2} (every step's chunks {:.2})",
                    self.virt.steps_out,
                    if self.virt.full {
                        "the numbering made again"
                    } else {
                        "the numbering as it was"
                    },
                    self.virt.resolved,
                    ms(self.virt_ns.1),
                    ms(self.virt_ns.0),
                )
            } else {
                String::new()
            },
            link_ms: ms(t_link),
            ready_ms: ms(st.placed_at),
            write_ms: ms(t_end - t_check),
        }
    }

    /// The spliced link of `changes`, deflate memoized by content (what
    /// the last few links used is kept: an edit undone finds its streams);
    /// with how many streams deflate compressed anew.
    fn spliced(
        &mut self,
        tex: &Tex<native::NativeHost, partex_core::ssa::SsaTracker>,
        changes: Vec<partex_core::effects::StepChunks>,
        virt: bool,
        clock: &dyn Fn() -> u64,
    ) -> (
        Result<Option<partex_core::effects::SpliceOut>, partex_core::effects::LinkError>,
        usize,
    ) {
        let links = self.links;
        let (mut was, mut now) = (
            std::mem::take(&mut self.deflated),
            std::collections::HashMap::new(),
        );
        let mut misses = 0usize;
        let mut deflate = |level: i32, data: &[u8]| {
            let key = partex_core::StableHasher::of(&(b"deflate", level, data));
            if let Some((z, _)) = now.get(&key) {
                return Some(Vec::clone(z));
            }
            let z = if let Some((z, _)) = was.remove(&key) {
                z
            } else {
                misses += 1;
                crate::zlib::deflate_stream(level, data)?
            };
            now.insert(key, (z.clone(), links));
            Some(z)
        };
        let linked = {
            let rec = tex.tracker().rec.borrow();
            let renumbered = partex_core::ssa::keys_renumbered(&rec);
            let key_of = |s: u32| partex_core::ssa::step_key(&rec, s);
            let keys: Option<&dyn Fn(u32) -> u64> =
                (renumbered != self.renumbered).then_some(&key_of);
            self.renumbered = renumbered;
            // (virtual numbers: the steps changed resolved, or every step
            // if the job's numbering moved, and the layout then made anew)
            let t0 = clock();
            let changes = if virt {
                if let Some(c) = self.virt.changes(&changes, &mut deflate) {
                    Some(c)
                } else {
                    let all = partex_core::ssa::all_step_chunks(&rec);
                    self.virt_ns.0 = clock() - t0;
                    let c = self.virt.full(&all, &mut deflate);
                    if c.as_ref().is_none_or(|x| x.1) {
                        self.splice.reset();
                    }
                    c.map(|x| x.0)
                }
            } else {
                Some(changes)
            };
            self.virt_ns.1 = clock() - t0;
            match changes {
                Some(c) => self.splice.link(c, keys, &mut deflate, clock),
                None => Ok(None),
            }
        };
        drop(deflate);
        was.retain(|_, (_, at)| *at + 8 > links);
        now.extend(was);
        self.deflated = now;
        (linked, misses)
    }

    /// The full link (`PARTEX_LINK_SPLICE=0`, or virtual object numbers):
    /// every step's chunks, every file written.
    fn link_full(
        &mut self,
        tex: &mut Tex<native::NativeHost, partex_core::ssa::SsaTracker>,
        origin: &std::time::Instant,
    ) -> LinkReport {
        self.link_full_once(tex, origin, true)
    }

    /// [`SsaLinker::link_full`], linking again once (`retry`) after a
    /// byte count's digits were rendered anew.
    #[allow(clippy::too_many_lines)]
    fn link_full_once(
        &mut self,
        tex: &mut Tex<native::NativeHost, partex_core::ssa::SsaTracker>,
        origin: &std::time::Instant,
        retry: bool,
    ) -> LinkReport {
        use partex_core::host::Host;
        #[allow(clippy::cast_precision_loss, reason = "a report")]
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
        #[allow(clippy::cast_precision_loss, reason = "a report")]
        let ns = |n: u64| n as f64 / 1e6;
        let t_fx = origin.elapsed();
        let chunks = partex_core::ssa::step_effects(&tex.tracker().rec.borrow());
        let t_fx = origin.elapsed().saturating_sub(t_fx);
        let threads = partex_incr::Threads::available();
        let keyed: Vec<(u64, &[partex_core::effects::Effect])> =
            chunks.iter().map(|(k, e)| (*k, &e.1[..])).collect();
        // (a chunk is resolved again if its version is not the one the last
        // link resolved)
        let last = std::mem::take(&mut self.resolved_versions);
        let versions: std::collections::HashMap<u64, u128> =
            chunks.iter().map(|(k, e)| (*k, e.0.0)).collect();
        let touched = |k: u64| last.get(&k).is_none_or(|v| versions.get(&k) != Some(v));
        // (deflate memoized by content, as the spliced link's)
        let links = self.links;
        let (mut was, mut now) = (
            std::mem::take(&mut self.deflated),
            std::collections::HashMap::new(),
        );
        let mut times = partex_core::effects::LinkTimes::default();
        let clock = || u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let linked = partex_core::effects::link_cached_timed(
            &keyed,
            &touched,
            &mut self.resolved,
            &threads,
            &mut |level, data| {
                let key = partex_core::StableHasher::of(&(b"deflate", level, data));
                if let Some((z, _)) = now.get(&key) {
                    return Some(Vec::clone(z));
                }
                let z = match was.remove(&key) {
                    Some((z, _)) => z,
                    None => crate::zlib::deflate_stream(level, data)?,
                };
                now.insert(key, (z.clone(), links));
                Some(z)
            },
            &clock,
            &mut times,
        );
        if linked.is_ok() {
            self.resolved_versions = versions;
        } else {
            self.resolved = partex_core::effects::LinkCache::default();
        }
        was.retain(|_, (_, at)| *at + 8 > links);
        now.extend(was);
        self.deflated = now;
        let l = match linked {
            Ok(l) => l,
            // (virtual object numbers: the engine's guess at a file's length
            // took another number of digits; the text that prints it is
            // rendered again with the length the link found, and linked
            // again: the file's own bytes do not depend on that text)
            Err(partex_core::effects::LinkError::LengthDigits { file, actual, .. })
                if retry
                    && partex_core::ssa::set_flow_length(
                        &mut tex.tracker().rec.borrow_mut(),
                        file.0,
                        actual,
                    ) =>
            {
                drop(chunks);
                let _ = partex_core::ssa::take_step_changes(&mut tex.tracker().rec.borrow_mut());
                return self.link_full_once(tex, origin, false);
            }
            Err(e) => {
                eprintln!("phitex: ssa: the link step failed: {e:?}");
                std::process::exit(3);
            }
        };
        // (the next spliced link lays everything out again, resolving every
        // step: this link took the steps' changes)
        self.splice.reset();
        self.virt.reset();
        let t_link = origin.elapsed();
        let removed = partex_core::ssa::removed_files(&tex.tracker().rec.borrow());
        let host = tex.host_mut();
        let (files, bytes) = self.write_full(host, &l);
        remove_files(host, &removed);
        host.term_write(&l.term);
        for d in &l.diagnostics {
            host.diagnostic(d);
        }
        self.pages.clone_from(&l.pages);
        let t_end = origin.elapsed();
        LinkReport {
            how: format!(
                "linked in full, {} chunks ({} resolved again); ms: link {:.2} (the effects {:.2}, \
                 numbering {:.2}, resolve {:.2}, layout {:.2}: object streams {:.2}, \
                 cross-reference {:.2}; lengths {:.2}, copy {:.2}); files written {:.2} \
                 ({files} files, {bytes} bytes)",
                chunks.len(),
                self.resolved.resolved,
                ms(t_link),
                ms(t_fx),
                ns(times.numbering),
                ns(times.resolve),
                ns(times.layout),
                ns(times.objstm),
                ns(times.xref),
                ns(times.lengths),
                ns(times.copy),
                ms(t_end.saturating_sub(t_link)),
            ),
            link_ms: ms(t_link),
            ready_ms: ms(t_link),
            write_ms: ms(t_end.saturating_sub(t_link)),
        }
    }

    /// Panic unless the spliced link is a full link's: its chunks the
    /// build's, in order, and every file's bytes and the terminal's text
    /// those of a full link.
    fn check(
        &self,
        tex: &Tex<native::NativeHost, partex_core::ssa::SsaTracker>,
        out: &partex_core::effects::SpliceOut,
        virt: bool,
    ) {
        let chunks = partex_core::ssa::step_effects(&tex.tracker().rec.borrow());
        let keys: Vec<(u32, u32, u128)> = chunks
            .iter()
            .map(|(k, e)| {
                (
                    u32::try_from(k >> 32).unwrap_or(u32::MAX),
                    u32::try_from(k & 0xffff_ffff).unwrap_or(u32::MAX),
                    e.0.0,
                )
            })
            .collect();
        // (virtual numbers: the splice's chunks are resolved, versioned
        // with their entries)
        let laid: Vec<(u32, u32, u128)> = self
            .splice
            .chunk_keys()
            .into_iter()
            .zip(&keys)
            .map(|(l, k)| if virt { (l.0, l.1, k.2) } else { l })
            .collect();
        assert!(
            keys.len() == self.splice.chunk_keys().len() && keys == laid,
            "the spliced link's chunks are not the build's"
        );
        let slices: Vec<&[partex_core::effects::Effect]> =
            chunks.iter().map(|(_, e)| &e.1[..]).collect();
        let full =
            partex_core::effects::link(&slices, &partex_core::Sequential, &mut |level, data| {
                crate::zlib::deflate_stream(level, data)
            })
            .expect("a full link");
        assert!(
            full.files.len() == out.files.len()
                && full.files.iter().all(|(f, b)| {
                    out.files.get(f).is_some_and(|x| x.0 == b.len() as u64)
                        && self.splice.file(*f) == *b
                }),
            "the spliced link's files are not a full link's"
        );
        let mut diagnostics = Vec::new();
        self.splice
            .each_diagnostic(&mut |d| diagnostics.push(d.clone()));
        assert!(
            full.term == out.term
                && full.opened == out.opened
                && full.closed == self.splice.closed()
                && full.diagnostics == diagnostics
                && full.pages == self.splice.pages(),
            "the spliced link's terminal text, opens, closes, diagnostics or pages are not a full link's"
        );
    }

    /// The files to write: each name with the engine's id and kind of its
    /// last open (the engine's handles closed: their bytes are the link's
    /// to write), and whether a step opened it since the last link (its
    /// open emptied it).
    fn names(
        &mut self,
        host: &mut native::NativeHost,
        opened: &[(
            partex_core::host::WriteId,
            Vec<u8>,
            partex_core::host::FileKind,
        )],
    ) -> Vec<(Vec<u8>, u32, partex_core::host::FileKind, bool)> {
        use partex_core::host::Host;
        let mut last: std::collections::BTreeMap<Vec<u8>, (u32, partex_core::host::FileKind)> =
            std::collections::BTreeMap::new();
        let mut anew: std::collections::BTreeSet<&[u8]> = std::collections::BTreeSet::new();
        for (id, name, kind) in opened {
            host.close(*id);
            last.insert(name.clone(), (id.0, *kind));
            if host.opened_after(*id, self.opened) {
                anew.insert(name);
            }
        }
        self.opened = host.opens();
        last.into_iter()
            .map(|(name, (id, kind))| {
                let a = anew.contains(&name[..]);
                (name, id, kind, a)
            })
            .collect()
    }

    /// Write the spliced link's files: a file whose bytes did not change
    /// and which is as last written is left; one that changed from a byte
    /// on, and is as last written, is written from there (and cut to its
    /// length); any other is written whole.
    fn write_spliced(
        &mut self,
        host: &mut native::NativeHost,
        out: &partex_core::effects::SpliceOut,
    ) -> Written {
        use std::io::{Seek, SeekFrom, Write};
        let mut w = Written::default();
        for (name, id, kind, anew) in self.names(host, &out.opened) {
            let (len, from) = out.files.get(&id).copied().unwrap_or((0, None));
            let n = native::with_suffix(&name, kind);
            let at = host.in_output_dir(&n).unwrap_or(n);
            let path = native::path(&at);
            let as_written = !anew
                && self.written.get(&name).is_some_and(|&(i, l, t)| {
                    i == id
                        && std::fs::metadata(&path)
                            .is_ok_and(|m| m.len() == l && m.modified().ok() == t)
                });
            let from = match (as_written, from) {
                (true, None) => continue,
                (true, Some(x)) => x,
                (false, _) => 0,
            };
            if self.atomic {
                // (the bytes before the first change as they are on disk,
                // then the link's: the whole file, renamed into place)
                let keep = usize::try_from(from).unwrap_or(usize::MAX);
                let mut buf = if from > 0 {
                    std::fs::read(&path).unwrap_or_default()
                } else {
                    Vec::new()
                };
                let from = if buf.len() >= keep { from } else { 0 };
                buf.truncate(usize::try_from(from).unwrap_or(usize::MAX));
                self.splice
                    .write_from(id, from, &mut |b| buf.extend_from_slice(b));
                buf.truncate(usize::try_from(len).unwrap_or(usize::MAX));
                if write_atomic(&path, &buf).is_err() {
                    continue;
                }
                if from > 0 {
                    w.patched += 1;
                } else {
                    w.whole += 1;
                }
                w.bytes += len - from;
                let t = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok());
                host.note_written(&at);
                self.outputs
                    .insert(at, usize::try_from(len).unwrap_or(usize::MAX));
                self.written.insert(name, (id, len, t));
                continue;
            }
            let file = if from == 0 {
                std::fs::File::create(&path)
            } else {
                std::fs::OpenOptions::new().write(true).open(&path)
            };
            let Ok(file) = file else {
                continue;
            };
            let mut file = std::io::BufWriter::with_capacity(1 << 16, file);
            if from > 0 && file.seek(SeekFrom::Start(from)).is_err() {
                continue;
            }
            self.splice.write_from(id, from, &mut |b| {
                let _ = file.write_all(b);
            });
            let Ok(file) = file.into_inner() else {
                continue;
            };
            if from > 0 {
                let _ = file.set_len(len);
                w.patched += 1;
            } else {
                w.whole += 1;
            }
            w.bytes += len - from;
            let t = file.metadata().ok().and_then(|m| m.modified().ok());
            drop(file);
            host.note_written(&at);
            self.outputs
                .insert(at, usize::try_from(len).unwrap_or(usize::MAX));
            self.written.insert(name, (id, len, t));
        }
        w
    }

    /// Write a full link's files, every one.
    fn write_full(
        &mut self,
        host: &mut native::NativeHost,
        l: &partex_core::effects::Linked,
    ) -> (usize, usize) {
        use partex_core::host::Host;
        let (mut files, mut bytes_out) = (0, 0);
        for (name, id, kind, _) in self.names(host, &l.opened) {
            let bytes = l.files.get(&id).map_or(&[][..], Vec::as_slice);
            if self.atomic && kind != partex_core::host::FileKind::XdvPipe {
                let n = native::with_suffix(&name, kind);
                let at = host.in_output_dir(&n).unwrap_or(n);
                if write_atomic(&native::path(&at), bytes).is_ok() {
                    host.note_written(&at);
                    self.outputs.insert(at, bytes.len());
                }
            } else if let Some((w, written)) = host.open_write(&name, kind) {
                host.write(w, bytes);
                host.close(w);
                host.note_written(&written);
                self.outputs.insert(written, bytes.len());
            }
            files += 1;
            bytes_out += bytes.len();
            // (the next spliced link writes it whole)
            self.written.remove(&name);
        }
        (files, bytes_out)
    }
}

/// The files a command removed after the job's last open of each, removed
/// again after a link wrote the job's files (DESIGN 3.7, "Commands").
fn remove_files(host: &native::NativeHost, names: &[Vec<u8>]) {
    for n in names {
        let p = host.in_output_dir(n).unwrap_or_else(|| n.clone());
        let _ = std::fs::remove_file(native::path(&p));
    }
}

/// The spliced link's report: what it put in and rendered, and its times
/// (`times`: the link, the check against a full link if made, the
/// writes, in ms).
fn splice_report(
    changes: usize,
    st: &partex_core::effects::SpliceStats,
    misses: usize,
    times: (f64, Option<f64>, f64),
    w: &Written,
) -> String {
    #[allow(clippy::cast_precision_loss, reason = "a report")]
    let ms = |ns: u64| ns as f64 / 1e6;
    format!(
        "{changes} steps changed, {} chunks put in, {} taken out, {} live{}; \
         {} object streams rendered ({} bytes), {} cross-reference sections \
         ({} entries), deflate {misses} new ({:.2} ms in all); ms: placed {:.2}, \
         link {:.2}{}; files written {:.2} ({} whole, {} from a byte on, \
         {} bytes)",
        st.chunks_in,
        st.chunks_out,
        st.chunks,
        if st.reindexed { ", moved" } else { "" },
        st.streams,
        st.stream_bytes,
        st.xrefs,
        st.xref_entries,
        ms(st.deflate_ns),
        ms(st.placed_at),
        times.0,
        times
            .1
            .map(|c| format!(", checked against a full link {c:.1}"))
            .unwrap_or_default(),
        times.2,
        w.whole,
        w.patched,
        w.bytes,
    )
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
                "phitex: ssa link {i} (step {}): {} writes {w:?}",
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
        "phitex: ssa check: {} rows not values yet: {}",
        r.not_values.len(),
        r.not_values.join("; ")
    );
    eprintln!(
        "phitex: ssa check: {} hits checked, {} uncovered parts",
        r.checked,
        v.len()
    );
    for (part, (n, first)) in v {
        eprintln!("phitex: ssa uncovered {n:7} {part} (first: {first})");
    }
    let stale = &rec.st.stale;
    eprintln!(
        "phitex: ssa check: table reads whose version was not the content: {}",
        if stale.is_empty() {
            "none".to_string()
        } else {
            format!("{stale:?}")
        }
    );
    for s in &rec.st.stale_first {
        eprintln!("phitex: ssa stale {s}");
    }
    for d in &r.write_diffs {
        eprintln!("phitex: ssa write differs {d}");
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
    eprintln!("phitex: ssa {build} routines: {}", routines.join(", "));
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
    eprintln!("phitex: ssa reads noted/verified by family:{line}");
}

fn run_memo(host: native::NativeHost, mut params: Params, command_line: &[u8]) -> i32 {
    // (`PARTEX_MEMO=1`: memoized macro calls; `=stats` also reports them)
    let memo = std::env::var("PARTEX_MEMO").unwrap_or_default();
    params.memo = !memo.is_empty() && memo != "0";
    let mut tex = Tex::new(host, Untracked, params);
    side_files_setup(&mut tex);
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
    side_files_write(&mut tex);
    if limit.is_some() {
        eprintln!(
            "phitex: memo last hit {}",
            String::from_utf8_lossy(&tex.memo_last_hit())
        );
    }
    if memo == "stats" || memo == "check" {
        eprintln!("phitex: memo {:?}", tex.memo_stats());
        for (name, [calls, hits, stale, rec, bad], why) in tex.memo_defs(
            std::env::var("PARTEX_MEMO_DEFS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(40),
        ) {
            eprintln!(
                "phitex: memo def {calls:9} calls {hits:8} hits {stale:8} stale {rec:7} rec {bad:3} bad {} {why}",
                String::from_utf8_lossy(&name)
            );
        }
        for (name, n) in tex.memo_stale_cells() {
            eprintln!(
                "phitex: memo stale {n:9} {}",
                String::from_utf8_lossy(&name)
            );
        }
        for d in tex.memo_read_diffs() {
            eprintln!("phitex: memo {}", String::from_utf8_lossy(&d));
        }
        for (old, new) in tex.memo_pending_diffs() {
            eprintln!("phitex: memo stored: {}", String::from_utf8_lossy(&old));
            eprintln!("phitex: memo fresh:  {}", String::from_utf8_lossy(&new));
        }
        for (name, what) in tex.memo_mismatches() {
            eprintln!(
                "phitex: memo check: {} differs in {what}",
                String::from_utf8_lossy(&name)
            );
        }
    }
    history
}
