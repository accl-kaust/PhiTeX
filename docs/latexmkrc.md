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

- `phitex build` and `phitex watch`, with TeX Live and with Shelf
  (`--shelf`): the search paths are kpathsea's in a TeX Live build, and
  searched on disk before the project's folder and Shelf in a Shelf build;
  `\write18` commands see them too, as they would under latexmk.
- Under `pdflatex` (phitex run by that name), no rc file is read, as
  pdflatex reads none.
