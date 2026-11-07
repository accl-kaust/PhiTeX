# latexmk's configuration files in PhiTeX

A project set up for latexmk builds the same with phitex. PhiTeX reads
latexmk's configuration files (`.latexmkrc`, `latexmkrc`) as data: it never
runs the Perl in them. It reads the small part of Perl people actually write
in these files, and for every line it does not apply it says, once a build,
the file, the line and why.

## Which files, in which order

As latexmk does (latexmk 4.8x on Linux and macOS), at most one file of each
level, the first that exists, later levels over earlier ones:

1. The system's: `$LATEXMKRCSYS` if set; else the first of `/etc/LatexMk`,
   `/opt/local/share/latexmk/LatexMk`, `/usr/local/share/latexmk/LatexMk`,
   `/usr/local/lib/latexmk/LatexMk`, then the same four folders'
   `latexmkrc`.
2. Yours: `$XDG_CONFIG_HOME/latexmk/latexmkrc` (`~/.config/latexmk/latexmkrc`
   when `XDG_CONFIG_HOME` is unset and `~/.config` exists), else
   `~/.latexmkrc`.
3. The project's: `.latexmkrc`, else `latexmkrc`, in the folder phitex is
   run in (latexmk's working directory). Parent folders are not searched,
   as latexmk does not search them. A file there can load another with
   `do "../.latexmkrc";` (or `require`), which PhiTeX follows.

`$auto_rc_use = 0;` in a file stops the levels after it, as in latexmk.

Under an editor (`phitex watch --lsp`) the folder is the one the editor
opened.

## What applies

By default PhiTeX applies only what helps the build find files:

| Setting | How it is written |
|---|---|
| `TEXINPUTS`, `BIBINPUTS`, `BSTINPUTS` | `$ENV{'TEXINPUTS'} = '../common/tex//:' . $ENV{'TEXINPUTS'};`, `.=`, `ensure_path('TEXINPUTS', "$root/common/tex//");` |
| `@default_files` | `@default_files = ('thesis.tex');`, `push @default_files, ...`: the main file when neither the command line nor a `phitex.toml` names one |

The search paths follow kpathsea's rules, as pdflatex reads them when
latexmk runs it: a trailing `//` searches the folder's subfolders too, and an
empty element (the trailing `:`) is where TeX Live's own search goes. A
path with no empty element has no default search, as in kpathsea. Relative
folders are the working directory's.

Every other latexmk setting (`$pdf_mode`, `$out_dir`, `$aux_dir`,
`$pdflatex` and its options, `$bibtex_use`, `$max_repeat`, ...) is said
once and not applied: phitex chooses the engine, keeps its own output folder
and options, runs BibTeX and biber when the document needs them, and runs
LaTeX until its files settle. For example:

```
warning: .latexmkrc:3: $pdf_mode not applied (phitex chooses the engine itself)
```

## Two modes: find (the default) and exact

`find` applies only the settings above. `exact` applies latexmk's other
settings that PhiTeX has a counterpart for:

| Setting | With `exact` |
|---|---|
| `$pdf_mode` | 1: pdflatex, 5: xelatex. 4 (lualatex) is said and not applied: LuaLaTeX is not supported in PhiTeX 1.0. 0, 2 and 3 (DVI, PostScript) are said and not applied. |
| `$pdflatex`, `$xelatex` | the program it runs (`$pdflatex = 'xelatex %O %S'` builds with xelatex); `-synctex=N` (SyncTeX on unless 0); `-shell-escape` (see below); `-interaction`, `-file-line-error`, `-halt-on-error` accepted, as phitex already works that way; any other option said and not applied. `set_tex_cmds('...')` too. |
| `$out_dir` | the output folder (relative to the working folder) |
| `$aux_dir` | applied only when it is `$out_dir`: phitex writes the auxiliary files beside the PDF |
| `$bibtex_use` | 1, 1.5, 2: what phitex does already; 0 is said and not applied |
| `$max_repeat` | 5 is phitex's limit, latexmk's default; another value is said and not applied |

Turn it on for one run with `--exact-latexmk`, or for good with
`phitex config latexmk exact` (or `latexmk = "exact"` in a project's
`phitex.toml`). The command line's own options stay over the rc files'
(`--engine`, `--output-dir`, `--synctex`).

`-shell-escape` from a `.latexmkrc` applies only with `--exact-latexmk`
typed on the command line: a project folder may be anyone's, and the
setting in `phitex.toml` (which a project can carry) never lets it run
commands. An `$out_dir` outside the working folder likewise needs the
flag. In the browser (the extension, Overleaf) `-shell-escape` from a
`.latexmkrc` never applies.

## phitex.toml's `inputs`

PhiTeX's own way to say where TeX looks first, without a `.latexmkrc`:

```toml
inputs = ["../common/tex//"]   # or one folder: inputs = "../common/tex//"
```

Each entry is a folder in kpathsea's syntax (a trailing `//`: its
subfolders too), relative to that `phitex.toml`'s folder. The entries go
before `TEXINPUTS` from the environment, which goes before TeX Live's own
search. A project's `inputs` are over yours (`~/.config/phitex/phitex.toml`,
relative to that file's folder).

## Which wins

Command-line options, then the `.latexmkrc` files, then `phitex.toml`, then
PhiTeX's defaults. A `.latexmkrc` sees `phitex.toml`'s `inputs` at the start
of `$ENV{TEXINPUTS}`: one that appends to `$ENV{TEXINPUTS}` keeps them,
one that sets it outright replaces them.

Why the `.latexmkrc` is over `phitex.toml`: it is the file every other tool
builds the project with (latexmk, Overleaf, editors that run latexmk), so its
result is the PDF people compare with; `phitex.toml` fills in what no
`.latexmkrc` says. One exception: `main` in `phitex.toml` names the main file
as the command line does, so `@default_files` applies only when neither
names one (a `@default_files` in your own `~/.latexmkrc` must not pick a
different file in a project that names its own).

## The Perl PhiTeX reads

- Assignments of strings, numbers and other variables, joined with `.`:
  `'single'` and `"double"` quotes (with `$var`, `${var}` and `$ENV{VAR}`
  inside double quotes), `q{...}`, `qq{...}`, `qw(...)`, `||` and `//`.
- `my`, `our` and `local` variables, used later in the same reading.
- `use File::Basename ...;`, `use Cwd ...;`, `use strict;`,
  `use warnings;`: accepted (they only import names).
- `dirname`, `basename`, `abs_path`, `realpath`, `getcwd`, `cwd`,
  `__FILE__`, `File::Spec->catfile`.
- latexmk's `ensure_path(VAR, PATH, ...)`: each path put before `$ENV{VAR}`
  unless it is already one of its elements; `PATH:` when `VAR` is unset.
- `do FILE;` and `require FILE;`: the file read in place. A relative name is
  the working directory's when it starts with `./` or `../`, as Perl takes
  it; a bare name (`do "x.rc"`) is looked for in Perl's `@INC`, which has no
  `.`, so latexmk does not find it either, and PhiTeX says so. A file
  already being read is not read again (a cycle).

`__FILE__` keeps Perl's meaning. latexmk reads a file it finds by
evaluating its text, so `__FILE__` there is `(eval 1)`, and
`dirname(abs_path(__FILE__))` is the working directory. In a file loaded
with `do`, `__FILE__` is the name `do` was given. So this common pair works
from the repository's root and from a lab folder alike:

```perl
# the root's .latexmkrc
use File::Basename qw(dirname);
use Cwd qw(abs_path);
my $root = dirname(abs_path(__FILE__));
ensure_path('TEXINPUTS', "$root/common/tex//");
```

```perl
# lab1/.latexmkrc
do "../.latexmkrc";
```

Anything else (subroutines, `if`, loops, `system`, `add_cus_dep`, other
modules) is not run, and its line is said with why.

## Where it applies

- The browser hosts (the extension, Overleaf through it): the project's
  `latexmkrc` or `.latexmkrc` at its root, as Overleaf reads it (no system
  or user file there). Its `TEXINPUTS` folders inside the project are
  searched; folders outside it are said and not searched. `BIBINPUTS` and
  `BSTINPUTS` are said and not applied there.

- `phitex build` and `phitex watch`, with TeX Live and with Shelf
  (`--shelf`): the search paths are kpathsea's in a TeX Live build, and
  searched on disk before the project's folder and Shelf in a Shelf build;
  `\write18` commands see them too, as they would under latexmk.
  `phitex watch` rebuilds when any file the build read changes, in
  whatever folder it was found (a style of a `TEXINPUTS` folder, a `.bib`
  of a `BIBINPUTS` one), and reads the rc files again when one of them
  (or a file one `do`es) changes, or one appears where latexmk looks:
  the next build searches the new paths.
- Under `pdflatex` (phitex run by that name), no rc file is read, as
  pdflatex reads none.
