//! The rc reader over realistic `.latexmkrc` files (written for these
//! tests: no third-party file is read or run).

use std::collections::{BTreeMap, HashMap};

use super::*;

/// Files in memory, by absolute path.
#[derive(Default)]
struct Mem(HashMap<String, String>);

impl Mem {
    fn with(mut self, path: &str, text: &str) -> Mem {
        self.0.insert(path.to_owned(), text.to_owned());
        self
    }
}

impl RcFiles for Mem {
    fn read(&self, path: &str) -> Option<String> {
        self.0.get(path).cloned()
    }
}

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// The working directory's rc files only, read in `cwd`.
fn here(cwd: &str, files: &Mem, e: &[(&str, &str)]) -> Rc {
    let w = Where {
        cwd: cwd.into(),
        ..Where::default()
    };
    read(&order(&w), cwd, &env(e), files)
}

#[test]
fn the_shared_style_folder_by_env_assignment() {
    let m = Mem::default().with(
        "/p/paper/.latexmkrc",
        "# shared styles\n$ENV{'TEXINPUTS'}='../common/tex//:' . $ENV{'TEXINPUTS'};\n",
    );
    let rc = here("/p/paper", &m, &[]);
    assert_eq!(rc.notes, vec![]);
    // (TEXINPUTS unset: '' after it, the trailing colon kpathsea's default)
    assert_eq!(rc.var("TEXINPUTS"), Some("/p/common/tex//:"));
    assert_eq!(rc.read, vec![".latexmkrc"]);
}

#[test]
fn an_env_texinputs_is_kept_after_the_rc_files_folder() {
    let m = Mem::default().with(
        "/p/latexmkrc",
        "$ENV{TEXINPUTS} = \"./sty//:\" . $ENV{TEXINPUTS};",
    );
    let rc = here("/p", &m, &[("TEXINPUTS", "/opt/tex//:")]);
    assert_eq!(rc.notes, vec![]);
    assert_eq!(rc.var("TEXINPUTS"), Some("/p/sty//:/opt/tex//:"));
}

#[test]
fn dotted_append_and_or_forms() {
    let m = Mem::default().with(
        "/p/.latexmkrc",
        "$ENV{'BIBINPUTS'} .= ':bib';\n\
         $ENV{'BSTINPUTS'} = 'bst:' . ($ENV{'BSTINPUTS'} || '');\n\
         $ENV{\"TEXINPUTS\"} = 'tex:' . ($ENV{TEXINPUTS} // '');\n",
    );
    let rc = here("/p", &m, &[]);
    assert_eq!(rc.notes, vec![]);
    assert_eq!(rc.var("BIBINPUTS"), Some(":/p/bib"));
    assert_eq!(rc.var("BSTINPUTS"), Some("/p/bst:"));
    assert_eq!(rc.var("TEXINPUTS"), Some("/p/tex:"));
}

/// The course repository's rc file (the user's), as written.
const COURSE_RC: &str = "\
# latexmk settings for the CS221P lab handouts.
# Puts common/tex/ (cs221p-lab.sty and the KAUST logos) on the TeX search
# path, so a lab builds from the repo root:
#     latexmk -pdf -outdir=/tmp/tex/lab1 lab1-setup/lab1.tex
# Lab folders have a small .latexmkrc that loads this one, so the same
# works from inside a lab folder:
#     cd lab1-setup && latexmk -pdf -outdir=/tmp/tex/lab1 lab1.tex
use File::Basename qw(dirname);
use Cwd qw(abs_path);
my $cs221p_root = dirname(abs_path(__FILE__));
ensure_path('TEXINPUTS', \"$cs221p_root/common/tex//\");
";

#[test]
fn the_course_repository_from_its_root() {
    let m = Mem::default().with("/r/.latexmkrc", COURSE_RC);
    let rc = here("/r", &m, &[]);
    assert_eq!(rc.notes, vec![]);
    // (latexmk evals the file: __FILE__ is `(eval N)`, abs_path of it is
    // in the working directory)
    assert_eq!(rc.var("TEXINPUTS"), Some("/r/common/tex//:"));
}

#[test]
fn the_course_repository_from_a_lab_folder() {
    for small in [
        "do \"../.latexmkrc\";\n",
        "do '../.latexmkrc';",
        "require \"../.latexmkrc\";",
        "# the course's settings\ndo('../.latexmkrc');\n",
    ] {
        let m = Mem::default()
            .with("/r/.latexmkrc", COURSE_RC)
            .with("/r/lab1-setup/.latexmkrc", small);
        let rc = here("/r/lab1-setup", &m, &[]);
        assert_eq!(rc.notes, vec![], "{small}");
        // (in a file `do`ne, __FILE__ is the name `do` was given)
        assert_eq!(rc.var("TEXINPUTS"), Some("/r/common/tex//:"), "{small}");
        assert_eq!(rc.read, vec![".latexmkrc", "/r/.latexmkrc"], "{small}");
    }
}

#[test]
fn an_include_by_file_relative_name() {
    let m = Mem::default()
        .with("/r/common.rc", "ensure_path('TEXINPUTS', './tex//');")
        .with(
            "/r/sub/latexmkrc",
            "use File::Basename;\nmy $here = dirname(__FILE__);\ndo \"$here/../common.rc\";\n",
        );
    let rc = here("/r/sub", &m, &[]);
    // (latexmk read sub/latexmkrc itself: __FILE__ `(eval 1)`, its dirname
    // `.`, so `./../common.rc` from /r/sub)
    assert_eq!(rc.notes, vec![]);
    assert_eq!(rc.var("TEXINPUTS"), Some("/r/sub/tex//:"));
}

#[test]
fn include_cycles_and_missing_files_are_said() {
    let m = Mem::default()
        .with(
            "/r/a/.latexmkrc",
            "do './.latexmkrc';\ndo '../none';\ndo 'bare.rc';",
        )
        .with("/r/x", "");
    let rc = here("/r/a", &m, &[]);
    let whys: Vec<_> = rc
        .notes
        .iter()
        .map(|n| (n.at.line, n.why.clone()))
        .collect();
    assert_eq!(whys.len(), 3, "{whys:?}");
    assert!(whys[0].1.contains("cycle"), "{whys:?}");
    assert!(whys[1].1.contains("no such file"), "{whys:?}");
    assert!(whys[2].1.contains("@INC"), "{whys:?}");
    assert_eq!((whys[0].0, whys[1].0, whys[2].0), (1, 2, 3));
}

#[test]
fn ensure_path_keeps_an_element_already_there() {
    let m = Mem::default().with(
        "/p/.latexmkrc",
        "ensure_path('TEXINPUTS', '/s//');\nensure_path( 'TEXINPUTS', '/s//', '/t' );",
    );
    let rc = here("/p", &m, &[]);
    assert_eq!(rc.notes, vec![]);
    assert_eq!(rc.var("TEXINPUTS"), Some("/t:/s//:"));
}

#[test]
fn the_order_system_user_project_later_over_earlier() {
    let w = Where {
        cwd: "/p".into(),
        home: Some("/h".into()),
        home_config_is_dir: true,
        system_and_user: true,
        ..Where::default()
    };
    let levels = order(&w);
    assert_eq!(levels.len(), 3);
    assert_eq!(levels[0][0], "/etc/LatexMk");
    assert_eq!(levels[0][4], "/etc/latexmkrc");
    assert_eq!(
        levels[1],
        vec!["/h/.config/latexmk/latexmkrc", "/h/.latexmkrc"]
    );
    assert_eq!(levels[2], vec!["/p/.latexmkrc", "/p/latexmkrc"]);
    let m = Mem::default()
        .with(
            "/etc/latexmkrc",
            "$ENV{TEXINPUTS} = '/sys:';\n$pdf_mode = 1;",
        )
        .with(
            "/h/.latexmkrc",
            "$ENV{TEXINPUTS} = '/user:' . $ENV{TEXINPUTS};",
        )
        // (only the first of the level: latexmkrc is not read beside .latexmkrc)
        .with("/p/.latexmkrc", "$pdf_mode = 5;")
        .with("/p/latexmkrc", "$ENV{TEXINPUTS} = '/never:';");
    let rc = read(&levels, "/p", &BTreeMap::new(), &m);
    assert_eq!(rc.var("TEXINPUTS"), Some("/user:/sys:"));
    assert_eq!(
        rc.exact,
        vec![(
            Exact::PdfMode("5".into()),
            Place {
                file: ".latexmkrc".into(),
                line: 1
            }
        )]
    );
    // (XDG's file first: ~/.latexmkrc then not read)
    let m = m
        .with("/h/.config/latexmk/latexmkrc", "$ENV{TEXINPUTS} = '/xdg:';")
        .with("/p/.latexmkrc", "");
    let rc = read(&levels, "/p", &BTreeMap::new(), &m);
    assert_eq!(rc.var("TEXINPUTS"), Some("/xdg:"));
    // (LATEXMKRCSYS names the system file)
    let w2 = Where {
        latexmkrcsys: Some("/x/rc".into()),
        ..w
    };
    assert_eq!(order(&w2)[0], vec!["/x/rc"]);
}

#[test]
fn auto_rc_use_zero_stops_the_lower_levels() {
    let w = Where {
        cwd: "/p".into(),
        home: Some("/h".into()),
        system_and_user: true,
        ..Where::default()
    };
    let m = Mem::default()
        .with("/h/.latexmkrc", "$auto_rc_use = 0;\n$ENV{TEXINPUTS}='/u:';")
        .with("/p/.latexmkrc", "$ENV{TEXINPUTS}='/p:';");
    let rc = read(&order(&w), "/p", &BTreeMap::new(), &m);
    assert_eq!(rc.var("TEXINPUTS"), Some("/u:"));
}

#[test]
fn default_files_and_their_forms() {
    let m = Mem::default().with(
        "/p/.latexmkrc",
        "@default_files = ('thesis.tex', \"slides\");\npush @default_files, 'extra.tex';",
    );
    let rc = here("/p", &m, &[]);
    assert_eq!(rc.notes, vec![]);
    let (f, at) = rc.default_files.expect("set");
    assert_eq!(f, vec!["thesis.tex", "slides", "extra.tex"]);
    assert_eq!(at.line, 2);
    let m = Mem::default().with("/p/.latexmkrc", "@default_files = qw(a.tex b.tex);");
    assert_eq!(
        here("/p", &m, &[]).default_files.map(|d| d.0),
        Some(vec!["a.tex".to_owned(), "b.tex".to_owned()])
    );
}

/// A typical thesis rc: what only `--exact-latexmk` applies is listed,
/// each once, at its last setting.
#[test]
fn a_thesis_rc_what_exact_applies() {
    let m = Mem::default().with(
        "/p/.latexmkrc",
        r#"# thesis
$pdf_mode = 1;
$pdflatex = 'pdflatex -shell-escape -synctex=1 -interaction=nonstopmode -file-line-error %O %S';
$out_dir = 'build';
$aux_dir = 'build';
$bibtex_use = 2;
$max_repeat = 7;
$pdf_mode = 5;
$xelatex = "xelatex -synctex=0 -8bit %O %S";
$pdf_previewer = 'zathura %O %S';
@generated_exts = (@generated_exts, 'glo');
add_cus_dep('glo', 'gls', 0, 'makeglossaries');
sub makeglossaries {
    system("makeglossaries '$_[0]'");
}
"#,
    );
    let rc = here("/p", &m, &[]);
    let names: Vec<_> = rc.exact.iter().map(|(e, _)| e.name()).collect();
    assert_eq!(
        names,
        vec![
            "$pdf_mode",
            "$pdflatex",
            "$out_dir",
            "$aux_dir",
            "$bibtex_use",
            "$max_repeat",
            "$xelatex"
        ]
    );
    assert_eq!(rc.exact[0].1.line, 8, "the last $pdf_mode");
    let lines: Vec<usize> = rc.notes.iter().map(|n| n.at.line).collect();
    assert_eq!(lines, vec![10, 11, 12, 13], "{:#?}", rc.notes);
    assert!(rc.notes[3].why.contains("runs no"), "{}", rc.notes[3].why);
    let said = rc.not_applied(Some("--exact-latexmk"));
    assert_eq!(said.len(), 7);
    assert_eq!(
        said[2],
        ".latexmkrc:4: $out_dir not applied (phitex keeps its own output folder; \
         --exact-latexmk applies it)"
    );
    let a = rc.exact();
    assert_eq!(a.engine.as_deref(), Some("xelatex"));
    assert_eq!(a.synctex, Some(false));
    assert_eq!(
        a.shell_escape, None,
        "the xelatex command has no -shell-escape"
    );
    assert_eq!(a.out_dir.as_ref().map(|d| d.0.as_str()), Some("build"));
    assert_eq!(a.notes.len(), 2, "{:#?}", a.notes);
    assert!(a.notes[0].why.contains("-8bit"), "{:#?}", a.notes);
    assert!(a.notes[1].why.contains("$max_repeat = 7"), "{:#?}", a.notes);
}

#[test]
fn exact_pdf_mode_values() {
    let one = |text: &str| {
        let m = Mem::default().with("/p/.latexmkrc", text);
        here("/p", &m, &[]).exact()
    };
    assert_eq!(one("$pdf_mode = 1;").engine.as_deref(), Some("pdflatex"));
    let a = one("$pdf_mode = 4;");
    assert_eq!(a.engine, None);
    assert!(a.notes[0].why.contains("LuaLaTeX is not supported"));
    assert!(one("$pdf_mode = 2;").notes[0].why.contains("PostScript"));
    // ($pdflatex running xelatex: latexmk's old trick)
    let a = one("$pdf_mode = 1; $pdflatex = 'xelatex -shell-escape %O %S';");
    assert_eq!(a.engine.as_deref(), Some("xelatex"));
    assert_eq!(a.shell_escape.map(|s| s.0), Some(true));
    let a = one("set_tex_cmds('--shell-escape %O');");
    assert_eq!(a.shell_escape.map(|s| s.0), Some(true));
}

#[test]
fn lines_not_understood_name_their_line() {
    let m = Mem::default().with(
        "/p/.latexmkrc",
        "$ENV{TEXINPUTS} = './a:';\n\
         if ($^O eq 'MSWin32') {\n  $x = 1;\n} else {\n  $y = 2;\n}\n\
         $ENV{'TEXINPUTS'} = $foo . ':';\n\
         $ENV{SOURCE_DATE_EPOCH} = 0;\n\
         use POSIX;\n\
         system('make');\n",
    );
    let rc = here("/p", &m, &[]);
    let at: Vec<usize> = rc.notes.iter().map(|n| n.at.line).collect();
    assert_eq!(at, vec![2, 7, 8, 9, 10], "{:#?}", rc.notes);
    assert!(rc.notes[1].why.contains("$foo"));
    assert!(rc.notes[2].why.contains("SOURCE_DATE_EPOCH"));
    // (the value from line 1 kept: line 7 not applied)
    assert_eq!(rc.var("TEXINPUTS"), Some("/p/a:"));
}

#[test]
fn comments_pod_strings_with_semicolons_and_end() {
    let m = Mem::default().with(
        "/p/.latexmkrc",
        "=pod\n$ENV{TEXINPUTS}='/pod:';\n=cut\n\
         $ENV{TEXINPUTS} = 'a;b:'; # kpathsea takes ; as :\n\
         __END__\n$ENV{TEXINPUTS}='/after:';\n",
    );
    let rc = here("/p", &m, &[]);
    assert_eq!(rc.notes, vec![]);
    assert_eq!(rc.var("TEXINPUTS"), Some("/p/a;b:"));
}

#[test]
fn unclosed_string_is_said() {
    let m = Mem::default().with("/p/.latexmkrc", "\n$ENV{TEXINPUTS} = './a:;\n");
    let rc = here("/p", &m, &[]);
    assert_eq!(rc.notes.len(), 1);
    assert_eq!(rc.notes[0].at.line, 2);
}

#[test]
fn absolute_elements_of_a_path() {
    assert_eq!(
        absolute_elements("../c//:!!/abs:$HOME/x:~/t::.:./s/", "/w/d"),
        "/w/c//:!!/abs:$HOME/x:~/t::/w/d:/w/d/s"
    );
}

#[test]
fn commands_read() {
    let c = command("/usr/bin/pdflatex --shell-escape -synctex=1 -interaction=batchmode %O %S");
    assert_eq!(c.program, "pdflatex");
    assert_eq!(c.shell_escape, Some(true));
    assert_eq!(c.synctex, Some(true));
    assert_eq!(c.others, Vec::<String>::new());
    let c = command("pdflatex -jobname=x -enable-write18 %O %S");
    assert_eq!(c.others, vec!["-jobname=x", "-enable-write18"]);
}
