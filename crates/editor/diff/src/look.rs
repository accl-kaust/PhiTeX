//! The markup's definitions: what `\DIFadd`, `\DIFdel`, `\DIFaddbegin`,
//! … do, put at the end of the new preamble.
//!
//! They define fresh names only, all in a private namespace (`\phitexdif…`,
//! letters only: no catcode changed), allocate no register (the few the
//! underlines need are `\countdef`'d, `\dimendef`'d, … at fixed numbers far
//! above any allocation), load no font and no package: nothing a paragraph
//! the diff left unchanged reads is changed by them, so a build of the
//! marked-up document started from the document's own build runs again
//! only what the markup touched. The look is latexdiff's:
//!
//! - colours by the engine's own colour stack (pdfTeX's `\pdfcolorstack`,
//!   LaTeX's stack where it made one; XeTeX's and DVI's `color push`
//!   specials, as xcolor's drivers write them), as RGB of the chosen
//!   colours (latexdiff's blue and red by default);
//! - underlines and strike-outs that break across lines: ulem's own code
//!   (ulem.sty 2019/11/18, Donald Arseneau), renamed into the namespace,
//!   its registers at fixed numbers, its wave drawn by the PDF's own
//!   strokes (a pdfTeX literal, XeTeX's `pdf:content`) instead of lasy6's
//!   character, which would load a font; a DVI build draws a rule;
//! - `\sf`, `\bf`, `\scriptsize`, `\footnotesize` as latexdiff's
//!   `\DeclareOldFontCommand`s make them, without redefining `\sf`, `\bf`.
//!
//! The types whose look is a package's own (change bars: changebar;
//! deleted text in footnotes: footmisc's stable footnotes) and the
//! subtypes that need one (zref, atbegshi) still load it, as latexdiff
//! does.

use crate::{Color, Markup, Options, Subtype};

/// ulem's underlining, renamed: each definition on one line (every line
/// of the preamble's extension is tagged `%DIF PREAMBLE`, after a space a
/// macro's body would keep).
const ULEM: &str = r"\mathchardef\phitexdifbox=31001\relax
\mathchardef\phitexdifhyphenbox=31002\relax
\mathchardef\phitexdifmarkbox=31003\relax
\skipdef\phitexdifskip=31001\relax
\dimendef\phitexdifheight=31001\relax
\dimendef\phitexdifpixel=31002\relax
\dimendef\phitexdifdepth=31003\relax
\countdef\phitexdifpe=31001\relax
\phitexdifheight=\maxdimen\relax
\phitexdifdepth=\maxdimen\relax
\phitexdifpixel=1in\relax\divide\phitexdifpixel 300\relax
\def\phitexdifthickness{.4pt}
\def\phitexdifempty{}
\gdef\phitexdifender{}
\def\phitexdifunegroup{}
\let\phitexdifLAhskip\hskip
\let\phitexdifLApenalty\penalty
\let\phitexdifLAspace\ \relax
\protected\def\phitexdifend*{\relax\relax}
\def\phitexdifULset{\phitexdifsetdepth\def\phitexdifleadtype{\leaders\hrule height\phitexdifheight depth\phitexdifdepth}\ifmmode\phitexdifdepth-4pt\fi\phitexdifheight-\phitexdifdepth\advance\phitexdifheight\phitexdifthickness\relax\phitexdifon}
\def\phitexdifsetdepth{\relax\ifdim\phitexdifdepth=\maxdimen\setbox\phitexdifbox\hbox{{(j}}\phitexdifdepth\dp\phitexdifbox\advance\phitexdifdepth.4pt\fi}
\def\phitexdifon{\ifmmode\expandafter\phitexdifonmath\else\expandafter\phitexdifonh\fi}
\long\def\phitexdifonh#1{\leavevmode\phitexdifender\let\phitexdifonh\phitexdifonin\everymath{\phitexdifhrest}\everyvbox{\phitexdifhrest}\let\hskip\phitexdifhskip\let\-\phitexdifdischyp\let\ \phitexdifspace\def\hfil{\hskip0pt plus1fil\relax}\def\hfill{\hskip0pt plus1fill\relax}\def\hss{\hskip0pt plus1fil minus1fil\relax}\let\penalty\phitexdifpenalty\phitexdifword\phitexdifempty#1\xdef\phitexdifspfactor{\the\spacefactor} \phitexdifend * }
\long\def\phitexdifonin#1{\leavevmode\phitexdifender\ifdim\phitexdifdepth=\maxdimen\else\phitexdifheight\phitexdifthickness\relax\advance\phitexdifdepth3\phitexdifheight\advance\phitexdifheight-\phitexdifdepth\fi\setbox\phitexdifbox\hbox{{#1}}\let\phitexdifstart\relax\phitexdifputbox\egroup}
\def\phitexdifonmath#1{\phitexdifender\mathord{\phitexdifhrest\mathop{\kern0pt#1}\limits\sb{\phitexdifleadtype\phitexdifLAhskip1pt plus1fill}}\egroup}
\long\def\phitexdifword#1 {\expandafter\phitexdifstart#1 \expandafter\ifx\expandafter\phitexdifend#1\egroup\egroup\unskip\unskip\unskip\spacefactor\phitexdifspfactor\relax\let\phitexdifword\egroup\else\ifmmode\else\ifdim\lastskip=0pt\else\global\phitexdifskip\lastskip\unskip\phitexdifstop\phitexdifleaders\fi\fi\fi\phitexdifword\phitexdifempty}
\def\phitexdifstart{\setbox\phitexdifbox\hbox\bgroup\everyhbox{\phitexdifhrest}\let\phitexdifstart\phitexdifempty\def\phitexdifunegroup{\bgroup\bgroup}\let\phitexdifleadtype\phitexdifempty\bgroup\kern-3sp\kern3sp\relax}
\def\phitexdifstop{\global\phitexdifpe\lastpenalty\unpenalty\ifnum\lastkern=3 \egroup\egroup\ifdim\wd\phitexdifbox=0pt \else\phitexdifputbox\fi\else\egroup\egroup\phitexdifputbox\fi\ifnum\phitexdifpe=0 \else\phitexdifLApenalty\phitexdifpe\fi\phitexdifunegroup}
\def\phitexdifputbox{\ifx\phitexdifstart\phitexdifempty\else\vrule width0pt\phitexdifLApenalty10000 {\phitexdifskip\wd\phitexdifbox\phitexdifleaders\kern-\phitexdifskip}\box\phitexdifbox\fi}
\def\phitexdifleaders{{\phitexdifLAhskip-\phitexdifpixel\advance\phitexdifskip2\phitexdifpixel\phitexdifleadtype\phitexdifLAhskip\phitexdifskip\phitexdifLAhskip-\phitexdifpixel}}
\def\phitexdifhrest{\let\ \phitexdifLAspace\let\-\phitexdifempty\let\penalty\phitexdifLApenalty}
\protected\def\phitexdifspace{\phitexdifLAspace\global\phitexdifskip\lastskip\unskip\phitexdifreskip}
\protected\def\phitexdifdischyp{\global\setbox\phitexdifhyphenbox\hbox{\ifnum\hyphenchar\font<0 \string-\else\char\hyphenchar\font\fi}\kern\wd\phitexdifhyphenbox\phitexdifLApenalty10000 \phitexdifstop\kern-\wd\phitexdifhyphenbox\discretionary{\box\phitexdifhyphenbox}{}{}\phitexdifstart}
\protected\def\phitexdifpenalty{\relax\ifhmode\afterassignment\phitexdifpenaltyb\count255=\else\phitexdifLApenalty\fi}
\def\phitexdifpenaltyb{\phitexdifLApenalty\ifnum\count255=0 1\else\count255\fi\relax\phitexdifstop\phitexdifstart}
\protected\def\phitexdifhskip{\ifx\ \phitexdifLAspace\phitexdifLAhskip\else\afterassignment\phitexdifreskip\global\phitexdifskip\fi}
\def\phitexdifreskip{\phitexdifstop\phitexdifleaders\phitexdifstart}
\protected\def\phitexdifmarkover#1{\leavevmode\setbox\phitexdifmarkbox\hbox{{#1}}\phitexdifpixel.5\wd\phitexdifmarkbox\ifmmode\setbox\phitexdifmarkbox\hbox{\raise1.4ex\box\phitexdifmarkbox}\dp\phitexdifmarkbox-1.4ex\ht\phitexdifmarkbox0pt\relax\def\phitexdifleadtype{\cleaders\copy\phitexdifmarkbox}\else\def\phitexdifleadtype{\leaders\copy\phitexdifmarkbox}\fi}
\def\phitexdifwaverule{\vrule width4bp height.2pt depth.2pt\relax}
\def\phitexdifwave{\ifcsname XeTeXrevision\endcsname\hbox to4bp{\special{pdf:content q 0.4 w 0 0 m 1.333 2 2.667 -2 4 0 c S Q}\hss}\else\ifx\pdfliteral\phitexdifundefined\phitexdifwaverule\else\ifnum\pdfoutput>0 \hbox to4bp{\pdfliteral{q 0.4 w 0 0 m 1.333 2 2.667 -2 4 0 c S Q}\hss}\else\phitexdifwaverule\fi\fi\fi}
\protected\def\phitexdifuwave{\leavevmode\bgroup\ifdim\phitexdifdepth=\maxdimen\phitexdifdepth3.5pt\relax\else\advance\phitexdifdepth2pt\relax\fi\phitexdifmarkover{\lower\phitexdifdepth\hbox{\phitexdifwave}}\phitexdifon}
\protected\def\phitexdifuline{\leavevmode\bgroup\phitexdifULset}
\protected\def\phitexdifsoutt{\leavevmode\bgroup\phitexdifdepth=-.55ex\relax\phitexdifULset}
\protected\long\def\phitexdifsout#1{\ifmmode\ifx\text\phitexdifundefined\mbox{\phitexdifsoutt{$#1$}}\else\text{\phitexdifsoutt{$#1$}}\fi\else\phitexdifsoutt{#1}\fi}";

/// The colours, by the engine's colour stack: `\phitexdifpush{R G B}`
/// and `\phitexdifpop` around a run (pdfTeX writing a PDF: its stack, as
/// LaTeX's colour makes it where it did; else the `color` specials, which
/// XeTeX's PDF and the DVI drivers read); `\phitexdifcontent{R G B}`
/// the colour for the rest of the group, which must end where the
/// coloured text does, LaTeX's `\\normalcolor` in it this colour too (a
/// caption's, the caption package's: a deleted caption stays deleted) (LaTeX's `\color` where the
/// document has it, so that its boxes, `\mbox`, a caption, keep it: else
/// pushed, `\phitexdifcpop` popping it); `\phitexdifmark{R G B}{text}` the
/// text in the colour, in a group (out of math, a paragraph begun: no
/// colour whatsit left in the vertical list), its spaces able to stretch
/// further than the text's (marked words are boxes, never hyphenated: a
/// line of them can still be filled, not run into the margin).
const COLOR: &str = r"\def\phitexdifstack{\ifcsname @pdfcolorstack\endcsname\csname @pdfcolorstack\endcsname\else0\fi}
\def\phitexdifpdf{\ifcsname XeTeXrevision\endcsname\else\ifx\pdfcolorstack\phitexdifundefined\else\ifnum\pdfoutput>0 1\fi\fi\fi}
\protected\def\phitexdifpush#1{\ifnum0\phitexdifpdf=1 \pdfcolorstack\phitexdifstack push{#1 rg #1 RG}\else\special{color push rgb #1}\fi}
\protected\def\phitexdifpop{\ifnum0\phitexdifpdf=1 \pdfcolorstack\phitexdifstack pop\else\special{color pop}\fi}
\def\phitexdifcommas#1 #2 #3\relax{#1,#2,#3}
\protected\def\phitexdifcontent#1{\edef\phitexdifx{#1}\ifdefined\color\edef\phitexdifx{\noexpand\color[rgb]{\expandafter\phitexdifcommas\phitexdifx\relax}}\phitexdifx\expandafter\let\csname default@color\expandafter\endcsname\csname current@color\endcsname\let\phitexdifcpop\relax\else\phitexdifpush{#1}\let\phitexdifcpop\phitexdifpop\fi}
\protected\long\def\phitexdifmark#1#2{\ifmmode\else\leavevmode\fi{\ifmmode\else\spaceskip\fontdimen2\font plus2\fontdimen2\font minus\fontdimen4\font\relax\fi\phitexdifcontent{#1}#2\phitexdifcpop}}";

/// Whole units in a frame (floats, verbatim, pictures, images): the
/// unit's box framed, `\phitexdifframe{box}{R G B}{dashed}{rule}{sep}`
/// setting `\phitexdiffbox` (the frame's baseline the unit's, `\phitexdiffd`
/// below the frame's bottom), dashed for deleted, solid for added; a block
/// (`\phitexdifblockbegin{R G B}{dashed}` … `\phitexdifblockend`) the
/// column's width, its text set 8pt narrower and coloured; one in the line
/// (`\phitexdifinlinebegin` …) an hbox. No argument holds the unit
/// (verbatim reads its own catcodes); the boxes and dimensions are at fixed
/// numbers, as the underlining's.
const BLOCKS: &str = r"\mathchardef\phitexdifbbox=31004\relax
\mathchardef\phitexdifibox=31005\relax
\mathchardef\phitexdiffbox=31006\relax
\dimendef\phitexdiffw=31004\relax
\dimendef\phitexdiffh=31005\relax
\dimendef\phitexdiffr=31006\relax
\dimendef\phitexdiffs=31007\relax
\dimendef\phitexdiffd=31008\relax
\def\phitexdifside#1{\ifnum#1=1 \vbox to\phitexdiffh{\xleaders\vbox to4pt{\vss\hrule width\phitexdiffr height2pt\vss}\vfil}\else\vrule width\phitexdiffr height\phitexdiffh depth0pt\fi}
\def\phitexdiftop#1{\ifnum#1=1 \hbox to\phitexdiffw{\xleaders\hbox to4pt{\hss\vrule width2pt height\phitexdiffr depth0pt\hss}\hfil}\else\hrule height\phitexdiffr depth0pt width\phitexdiffw\fi}
\protected\def\phitexdifframe#1#2#3#4#5{\phitexdiffr#4\relax\phitexdiffs#5\relax\phitexdiffd\dp#1\advance\phitexdiffd\phitexdiffs\advance\phitexdiffd\phitexdiffr\setbox\phitexdiffbox\vbox{\kern\phitexdiffs\hbox{\kern\phitexdiffs\box#1\kern\phitexdiffs}\kern\phitexdiffs}\phitexdiffw\wd\phitexdiffbox\phitexdiffh\ht\phitexdiffbox\advance\phitexdiffh2\phitexdiffr\setbox\phitexdiffbox\hbox{\phitexdifpush{#2}\phitexdifside{#3}\vbox{\baselineskip-1000pt\lineskip0pt\lineskiplimit\maxdimen\phitexdiftop{#3}\box\phitexdiffbox\phitexdiftop{#3}}\phitexdifside{#3}\phitexdifpop}}
\protected\def\phitexdifblockbegin#1#2{\par\begingroup\def\phitexdifbc{#1}\def\phitexdifbd{#2}\setbox\phitexdifbbox\vbox\bgroup\advance\hsize-8pt\relax\ifdefined\linewidth\advance\linewidth-8pt\relax\fi\ifdefined\columnwidth\advance\columnwidth-8pt\relax\fi\ifdefined\textwidth\advance\textwidth-8pt\relax\fi\begingroup\phitexdifcontent{#1}}
\protected\def\phitexdifblockend{\par\phitexdifcpop\endgroup\egroup\phitexdifframe\phitexdifbbox\phitexdifbc\phitexdifbd{.6pt}{3.4pt}\box\phitexdiffbox\endgroup}
\protected\def\phitexdifinlinebegin#1#2{\leavevmode\begingroup\def\phitexdifbc{#1}\def\phitexdifbd{#2}\setbox\phitexdifibox\hbox\bgroup\begingroup\phitexdifcontent{#1}}
\protected\def\phitexdifinlineend{\phitexdifcpop\endgroup\egroup\phitexdifframe\phitexdifibox\phitexdifbc\phitexdifbd{.4pt}{1.5pt}\lower\phitexdiffd\box\phitexdiffbox\endgroup}";

/// latexdiff's old font commands, as `\DeclareOldFontCommand` makes
/// `\sf` and `\bf` (in math, the math alphabet), and a size out of math.
const FONTS: &str = r"\protected\long\def\phitexdifsf#1{\ifmmode\mathsf{#1}\else{\normalfont\sffamily#1}\fi}
\protected\long\def\phitexdifbf#1{\ifmmode\mathbf{#1}\else{\normalfont\bfseries#1}\fi}
\protected\long\def\phitexdifsize#1#2{\ifmmode#2\else{#1#2}\fi}";

/// latexdiff's footnotes of deleted text, stable in moving arguments.
const FOOTMISC: &str = r"\RequirePackage[stable]{footmisc}";

/// A colour as the colour stack takes it: `R G B`, each from 0 to 1.
fn rgb(color: Option<&Color>, default: (f64, f64, f64)) -> String {
    let (red, green, blue) = color.and_then(Color::rgb).unwrap_or(default);
    let num = |x: f64| {
        let s = format!("{:.4}", x.clamp(0.0, 1.0));
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s.is_empty() {
            "0".to_owned()
        } else {
            s.to_owned()
        }
    };
    format!("{} {} {}", num(red), num(green), num(blue))
}

const BLUE: (f64, f64, f64) = (0.0, 0.0, 1.0);
const RED: (f64, f64, f64) = (1.0, 0.0, 0.0);

/// The definitions of `\DIFadd` (`add`: its name, `\DIFaddtex` under
/// hyperref) and `\DIFdel` for `m`, colours `a` and `d` (`R G B`), and the
/// packages and private definitions they need first.
#[allow(
    clippy::too_many_lines,
    reason = "one arm a type, as latexdiff lists them"
)]
fn markup(m: Markup, add: &str, del: &str, a: &str, d: &str, driver: &str) -> Vec<String> {
    let mut l: Vec<String> = Vec::new();
    let bar = format!("\\RequirePackage[{driver}]{{changebar}}");
    let ulem = || ULEM.lines().map(str::to_owned).collect::<Vec<_>>();
    let core = |l: &mut Vec<String>, u: bool| {
        if u {
            l.extend(ulem());
        }
        l.extend(COLOR.lines().map(str::to_owned));
        l.extend(FONTS.lines().map(str::to_owned));
    };
    let def = |name: &str, body: String| format!("\\providecommand{{\\{name}}}[1]{{{body}}}");
    let note = "[..\\footnote{removed: #1} ]";
    match m {
        Markup::Underline => {
            core(&mut l, true);
            l.push(def(
                add,
                format!("\\phitexdifmark{{{a}}}{{\\phitexdifuline{{#1}}}}"),
            ));
            l.push(def(
                del,
                format!("\\phitexdifmark{{{d}}}{{\\phitexdifsout{{#1}}}}"),
            ));
        }
        Markup::Ctraditional => {
            l.push(FOOTMISC.into());
            core(&mut l, false);
            l.push(def(
                add,
                format!("\\phitexdifmark{{{a}}}{{\\phitexdifsf{{#1}}}}"),
            ));
            l.push(def(del, format!("\\phitexdifmark{{{d}}}{{{note}}}")));
        }
        Markup::Traditional => {
            l.push(FOOTMISC.into());
            core(&mut l, false);
            l.push(def(add, "\\phitexdifsf{#1}".into()));
            l.push(def(del, format!("{{{note}}}")));
        }
        Markup::Cfont => {
            core(&mut l, false);
            l.push(def(
                add,
                format!("\\phitexdifmark{{{a}}}{{\\phitexdifsf{{#1}}}}"),
            ));
            l.push(def(
                del,
                format!("\\phitexdifmark{{{d}}}{{\\phitexdifsize\\scriptsize{{#1}}}}"),
            ));
        }
        Markup::Fontstrike => {
            core(&mut l, true);
            l.push(def(add, "\\phitexdifsf{#1}".into()));
            l.push(def(
                del,
                "\\phitexdifsize\\footnotesize{\\phitexdifsout{#1}}".into(),
            ));
        }
        Markup::Cchangebar => {
            l.push(bar);
            core(&mut l, false);
            l.push(def(
                add,
                format!("\\protect\\cbstart\\phitexdifmark{{{a}}}{{#1}}\\protect\\cbend"),
            ));
            l.push(def(
                del,
                format!("\\protect\\cbdelete\\phitexdifmark{{{d}}}{{#1}}\\protect\\cbdelete"),
            ));
        }
        Markup::Cfontchbar => {
            l.push(bar);
            core(&mut l, false);
            l.push(def(
                add,
                format!(
                    "\\protect\\cbstart\\phitexdifmark{{{a}}}{{\\phitexdifsf{{#1}}}}\\protect\\cbend"
                ),
            ));
            l.push(def(
                del,
                format!(
                    "\\protect\\cbdelete\\phitexdifmark{{{d}}}{{\\phitexdifsize\\scriptsize{{#1}}}}\\protect\\cbdelete"
                ),
            ));
        }
        Markup::Culinechbar => {
            l.push(bar);
            core(&mut l, true);
            l.push(def(
                add,
                format!(
                    "\\protect\\cbstart\\phitexdifmark{{{a}}}{{\\phitexdifuline{{#1}}}}\\protect\\cbend"
                ),
            ));
            l.push(def(
                del,
                format!(
                    "\\protect\\cbdelete\\phitexdifmark{{{d}}}{{\\phitexdifsout{{#1}}}}\\protect\\cbdelete"
                ),
            ));
        }
        Markup::Changebar => {
            l.push(bar);
            l.push(def(add, "\\protect\\cbstart{#1}\\protect\\cbend".into()));
            l.push(def(del, "\\protect\\cbdelete".into()));
        }
        Markup::Invisible => {
            l.push(def(add, "#1".into()));
            l.push(def(del, String::new()));
        }
        Markup::Bold => {
            core(&mut l, false);
            l.push(def(add, "\\phitexdifbf{#1}".into()));
            l.push(def(del, String::new()));
        }
    }
    l
}

/// The definitions of `\DIFaddbegin`, … for `s`, colours `a` and `d`.
/// `colored`: the colour definitions are there already.
#[allow(
    clippy::many_single_char_names,
    reason = "the colours a and d, as markup's"
)]
fn subtype(s: Subtype, a: &str, d: &str, colored: bool) -> Vec<String> {
    let cmd = |name: &str, body: &str| format!("\\providecommand{{\\{name}}}{{{body}}}");
    let six = |b: [&str; 6]| {
        [
            "DIFaddbegin",
            "DIFaddend",
            "DIFdelbegin",
            "DIFdelend",
            "DIFmodbegin",
            "DIFmodend",
        ]
        .iter()
        .zip(b)
        .map(|(n, body)| cmd(n, body))
        .collect::<Vec<_>>()
    };
    match s {
        Subtype::Safe => six(["", "", "", "", "", ""]),
        Subtype::Color => {
            let mut l: Vec<String> = Vec::new();
            if !colored {
                l.extend(COLOR.lines().map(str::to_owned));
            }
            let (pa, pd) = (
                format!("\\phitexdifpush{{{a}}}"),
                format!("\\phitexdifpush{{{d}}}"),
            );
            l.extend(six([&pa, "\\phitexdifpop", &pd, "\\phitexdifpop", "", ""]));
            l
        }
        Subtype::Margin => six([
            "\\protect\\marginpar{a[}",
            "\\protect\\marginpar{]}",
            "\\protect\\marginpar{d[}",
            "\\protect\\marginpar{]}",
            "\\protect\\marginpar{m[}",
            "\\protect\\marginpar{]}",
        ]),
        Subtype::Label | Subtype::Zlabel => {
            let z = s == Subtype::Zlabel;
            let mut l: Vec<String> = if z { ZLABEL_HEAD } else { LABEL_HEAD }
                .lines()
                .map(str::to_owned)
                .collect();
            l.push("\\gdef\\phitexdifcountb{0}\\gdef\\phitexdifcounte{0}".into());
            let label = if z { "\\zlabel" } else { "\\label" };
            let b = format!(
                "\\xdef\\phitexdifcountb{{\\the\\numexpr\\phitexdifcountb+1\\relax}}{label}{{DIFchgb\\phitexdifcountb}}"
            );
            let e = format!(
                "\\xdef\\phitexdifcounte{{\\the\\numexpr\\phitexdifcounte+1\\relax}}{label}{{DIFchge\\phitexdifcounte}}"
            );
            l.extend(six([&b, &e, &b, &e, &b, &e]));
            l
        }
        Subtype::OnlyChangedPage => ONLYCHANGEDPAGE.lines().map(str::to_owned).collect(),
    }
}

/// The definitions of the whole units' markup (floats, alignments,
/// pictures, verbatim, display math, images) for look `m`, colours `a` and
/// `d` (black where the look colours nothing; a deleted unit's frame
/// dashed, an added one's solid); `colored`: the colour definitions are
/// there already.
fn units(m: Markup, a: &str, d: &str, colored: bool) -> Vec<String> {
    let mut l: Vec<String> = Vec::new();
    if !colored {
        l.extend(COLOR.lines().map(str::to_owned));
    }
    l.extend(BLOCKS.lines().map(str::to_owned));
    let (a, d) = if m.colored() {
        (a, d)
    } else {
        ("0 0 0", "0 0 0")
    };
    let cmd = |name: &str, body: String| format!("\\providecommand{{\\{name}}}{{{body}}}");
    for (s, c, dash) in [("add", a, 0), ("del", d, 1)] {
        l.push(cmd(
            &format!("DIFblock{s}begin"),
            format!("\\phitexdifblockbegin{{{c}}}{{{dash}}}"),
        ));
        l.push(cmd(
            &format!("DIFblock{s}end"),
            "\\phitexdifblockend".into(),
        ));
        l.push(cmd(
            &format!("DIFinline{s}begin"),
            format!("\\phitexdifinlinebegin{{{c}}}{{{dash}}}"),
        ));
        l.push(cmd(
            &format!("DIFinline{s}end"),
            "\\phitexdifinlineend".into(),
        ));
        l.push(cmd(
            &format!("DIFmath{s}begin"),
            format!("\\phitexdifpush{{{c}}}"),
        ));
        l.push(cmd(&format!("DIFmath{s}end"), "\\phitexdifpop".into()));
        l.push(cmd(
            &format!("DIFcolor{s}begin"),
            format!("\\begingroup\\phitexdifcontent{{{c}}}"),
        ));
        l.push(cmd(
            &format!("DIFcolor{s}end"),
            "\\phitexdifcpop\\endgroup".into(),
        ));
    }
    l.push("\\mathchardef\\phitexdiftbox=31007\\relax".into());
    l.push(
        "\\protected\\def\\phitexdiftabbegin{\\leavevmode\\setbox\\phitexdiftbox\\hbox\\bgroup}"
            .into(),
    );
    l.push("\\protected\\def\\phitexdiftabend{\\egroup\\ifdim\\wd\\phitexdiftbox>\\linewidth\\ifdefined\\resizebox\\resizebox{\\linewidth}{!}{\\box\\phitexdiftbox}\\else\\box\\phitexdiftbox\\fi\\else\\box\\phitexdiftbox\\fi}".into());
    l.push("\\providecommand{\\DIFtabularbegin}{\\phitexdiftabbegin}".into());
    l.push("\\providecommand{\\DIFtabularend}{\\phitexdiftabend}".into());
    l.push("\\protected\\def\\phitexdifchangesep{\\ifhmode\\hskip.2em\\relax\\fi}".into());
    l.push("\\providecommand{\\DIFchangesep}{\\phitexdifchangesep}".into());
    l.push("\\providecommand{\\DIFfloatnumber}[1]{\\edef\\phitexdifsavedcount{\\the\\value{\\csname @captype\\endcsname}}\\setcounter{\\csname @captype\\endcsname}{#1}}".into());
    l.push("\\providecommand{\\DIFfloatrestore}{\\setcounter{\\csname @captype\\endcsname}{\\phitexdifsavedcount}}".into());
    l
}

/// The markup's lines for `opts` and a new preamble `preamble` (as
/// latexdiff's, each set under its `%DIF … PREAMBLE` line), in two: the
/// packages to load (at the preamble's end), and the rest, its
/// definitions (after `\begin{document}`: a preamble changed is a
/// document whose every paragraph its build runs again, the body's first
/// lines are not).
fn parts(opts: &Options, preamble: &str) -> (Vec<String>, Vec<String>) {
    let hyperref = crate::loads(preamble, &["hyperref"]);
    let a = rgb(opts.add_color.as_ref(), BLUE);
    let d = rgb(opts.del_color.as_ref(), RED);
    let (add, del) = if hyperref {
        ("DIFaddtex", "DIFdeltex")
    } else {
        ("DIFadd", "DIFdel")
    };
    let m = opts.markup;
    let mut lines = vec![format!("%DIF {} PREAMBLE", m.name())];
    let mut types = markup(m, add, del, &a, &d, opts.driver.name());
    // (the change bars in the added text's colour, where colours are
    // chosen: changebar's own, by xcolor)
    if m.bars() && (opts.add_color.is_some() || opts.del_color.is_some()) {
        let c = opts
            .add_color
            .clone()
            .unwrap_or_else(|| Color("blue".into()));
        types.insert(
            1,
            format!(
                "\\RequirePackage{{xcolor}}{}\\cbcolor{{DIFaddcolor}}",
                c.define("DIFaddcolor")
            ),
        );
    }
    lines.extend(types);
    let mut colored = lines
        .iter()
        .any(|l| l.starts_with("\\protected\\def\\phitexdifpush"));
    lines.push(format!("%DIF {} PREAMBLE", opts.subtype.name()));
    lines.extend(subtype(opts.subtype, &a, &d, colored));
    colored = colored || opts.subtype == Subtype::Color;
    if opts.subtype.identical_floats() {
        lines.push("%DIF IDENTICAL PREAMBLE".into());
        lines.extend(IDENTICAL.lines().map(str::to_owned));
    } else {
        lines.push("%DIF FLOATSAFE PREAMBLE".into());
        if m.footnotes() {
            // (a footnote cannot be in a float's caption or cell: deleted
            // text there shown small, as latexdiff's TRADITIONALSAFE shows
            // it)
            let small = "[..{\\scriptsize {removed: #1}} ]";
            lines.push(if m == Markup::Ctraditional {
                format!("\\providecommand{{\\DIFdelFL}}[1]{{\\phitexdifmark{{{d}}}{{{small}}}}}")
            } else {
                format!("\\providecommand{{\\DIFdelFL}}[1]{{{{{small}}}}}")
            });
        }
        lines.extend(FLOATSAFE.lines().map(str::to_owned));
    }
    lines.push("%DIF UNITS PREAMBLE".into());
    lines.extend(units(m, &a, &d, colored));
    if hyperref {
        lines.push("%DIF HYPERREF PREAMBLE".into());
        lines.extend(HYPERREF.lines().map(str::to_owned));
    }
    // (the packages; a subtype that redefines LaTeX's own commands as
    // it loads them, whole)
    let (mut pre, mut body) = (Vec::new(), Vec::new());
    let mut whole = false;
    for l in lines {
        if l.starts_with("%DIF ") {
            whole = l == "%DIF ONLYCHANGEDPAGE PREAMBLE";
            body.push(l);
        } else if whole || l.starts_with("\\RequirePackage") || l.starts_with("\\usepackage") {
            pre.push(l);
        } else {
            body.push(l);
        }
    }
    (pre, body)
}

/// `lines` tagged `%DIF PREAMBLE` each, between latexdiff's marks.
fn block(lines: &[String]) -> String {
    let mut s = String::from("%DIF PREAMBLE EXTENSION ADDED BY LATEXDIFF\n");
    for l in lines {
        s.push_str(l);
        s.push_str(" %DIF PREAMBLE\n");
    }
    s.push_str("%DIF END PREAMBLE EXTENSION ADDED BY LATEXDIFF\n");
    s
}

/// What the markup puts at the end of the new preamble (`preamble`), for
/// `opts`: the packages its look needs (changebar, footmisc, zref), each
/// line tagged `%DIF PREAMBLE` as latexdiff tags them; none for the other
/// looks (the default's among them).
#[must_use]
pub fn dif_preamble(opts: &Options, preamble: &str) -> String {
    let (pre, _) = parts(opts, preamble);
    if pre.is_empty() {
        String::new()
    } else {
        block(&pre)
    }
}

/// What the markup puts right after `\begin{document}`: its definitions
/// (latexdiff puts them in the preamble; here a document's build started
/// from its own sees no preamble changed), a comment first, so that the
/// line `\begin{document}` ends as it did.
#[must_use]
pub fn dif_body(opts: &Options, preamble: &str) -> String {
    let (_, body) = parts(opts, preamble);
    format!("%\n{}", block(&body))
}

const LABEL_HEAD: &str = r#"% To show only pages with changes (pdf) (external program pdftk needs to be installed)
% (only works for simple documents with non-repeated page numbers, otherwise use ZLABEL)
% pdflatex diff.tex
% pdflatex diff.tex
%pdftk diff.pdf cat \
%`perl -lne '\
% if (m/\\newlabel{DIFchg[b](\d*)}{{.*}{(.*)}}/) { $start{$1}=$2; print $2}\
% if (m/\\newlabel{DIFchg[e](\d*)}{{.*}{(.*)}}/) { \
%      if (defined($start{$1})) { \
%         for ($j=$start{$1}; $j<=$2; $j++) {print "$j";}\
%      } else { \
%         print "$2"\
%      }\
% }' diff.aux \
% | uniq \
% | tr  \\n ' '` \
% output diff-changedpages.pdf
% To show only pages with changes (dvips/dvipdf)
% dvips -pp `\
% [ put here the perl script from above]
% | uniq | tr -s \\n ','`
\typeout{Check comments in preamble of output for instructions how to show only pages where changes have been made}"#;

const ZLABEL_HEAD: &str = r#"% To show only pages with changes (pdf) (external program pdftk needs to be installed)
% (uses zref for reference to absolute page numbers)
% pdflatex diff.tex
% pdflatex diff.tex
%pdftk diff.pdf cat \
%`perl -lne 'if (m/\\zref\@newlabel{DIFchgb(\d*)}{.*\\abspage{(\d*)}}/ ) { $start{$1}=$2; print $2 } \
%  if (m/\\zref\@newlabel{DIFchge(\d*)}{.*\\abspage{(\d*)}}/) { \
%      if (defined($start{$1})) { \
%         for ($j=$start{$1}; $j<=$2; $j++) {print "$j";}\
%      } else { \
%         print "$2"\
%      }\
% }' diff.aux \
% | uniq \
% | tr  \\n ' '` \
% output diff-changedpages.pdf
% To show only pages with changes (dvips/dvipdf)
% latex diff.tex
% latex diff.tex
% dvips -pp `perl -lne 'if (m/\\newlabel{DIFchg[be]\d*}{{.*}{(.*)}}/) { print $1 }' diff.aux | uniq | tr -s \\n ','` diff.dvi
\typeout{Check comments in preamble of output for instructions how to show only pages where changes have been made}
\usepackage[user,abspage]{zref}"#;

const ONLYCHANGEDPAGE: &str = r"\RequirePackage{atbegshi}
\RequirePackage{etoolbox}
\RequirePackage{zref}
% redefine label command to write immediately to aux file - page references will be lost
\makeatletter \let\oldlabel\label% Store \label
\renewcommand{\label}[1]{% Update \label to write to the .aux immediately
\zref@wrapper@immediate{\oldlabel{#1}}}
\makeatother
\newbool{DIFkeeppage}
\newbool{DIFchange}
\boolfalse{DIFkeeppage}
\boolfalse{DIFchange}
\AtBeginShipout{%
  \ifbool{DIFkeeppage}
        {\global\boolfalse{DIFkeeppage}}  % True DIFkeeppage
         {\ifbool{DIFchange}{\global\boolfalse{DIFkeeppage}}{\global\boolfalse{DIFkeeppage}\AtBeginShipoutDiscard}} % False DIFkeeppage
}
\providecommand{\DIFaddbegin}{\global\booltrue{DIFkeeppage}\global\booltrue{DIFchange}}
\providecommand{\DIFaddend}{\global\booltrue{DIFkeeppage}\global\boolfalse{DIFchange}}
\providecommand{\DIFdelbegin}{\global\booltrue{DIFkeeppage}\global\booltrue{DIFchange}}
\providecommand{\DIFdelend}{\global\booltrue{DIFkeeppage}\global\boolfalse{DIFchange}}
\providecommand{\DIFmodbegin}{\global\booltrue{DIFkeeppage}\global\booltrue{DIFchange}}
\providecommand{\DIFmodend}{\global\booltrue{DIFkeeppage}\global\boolfalse{DIFchange}}";

const FLOATSAFE: &str = r"\providecommand{\DIFaddFL}[1]{\DIFadd{#1}}
\providecommand{\DIFdelFL}[1]{\DIFdel{#1}}
\providecommand{\DIFaddbeginFL}{}
\providecommand{\DIFaddendFL}{}
\providecommand{\DIFdelbeginFL}{}
\providecommand{\DIFdelendFL}{}";

const IDENTICAL: &str = r"\providecommand{\DIFaddFL}[1]{\DIFadd{#1}}
\providecommand{\DIFdelFL}[1]{\DIFdel{#1}}
\providecommand{\DIFaddbeginFL}{\DIFaddbegin}
\providecommand{\DIFaddendFL}{\DIFaddend}
\providecommand{\DIFdelbeginFL}{\DIFdelbegin}
\providecommand{\DIFdelendFL}{\DIFdelend}";

const HYPERREF: &str = r"\providecommand{\DIFadd}[1]{\texorpdfstring{\DIFaddtex{#1}}{#1}}
\providecommand{\DIFdel}[1]{\texorpdfstring{\DIFdeltex{#1}}{}}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_as_the_stack_takes_them() {
        assert_eq!(rgb(None, BLUE), "0 0 1");
        assert_eq!(
            rgb(Color::parse("#ff8000").ok().as_ref(), BLUE),
            "1 0.502 0"
        );
        assert_eq!(
            rgb(Color::parse("blue!60!black").ok().as_ref(), RED),
            "0 0 0.6"
        );
        assert_eq!(rgb(Color::parse("red!50").ok().as_ref(), BLUE), "1 0.5 0.5");
        assert_eq!(rgb(Color::parse("nosuchcolor").ok().as_ref(), RED), "1 0 0");
    }

    /// The default look defines only the latexdiff names (each provided:
    /// a document's own kept) and fresh private ones: no package, no
    /// allocation, no font, no catcode; every definition on one line.
    #[test]
    fn the_default_look_loads_nothing() {
        let o = Options {
            add_color: Color::parse("#0000ff").ok(),
            del_color: Color::parse("#ff0000").ok(),
            ..Options::default()
        };
        assert_eq!(dif_preamble(&o, "\\usepackage{amsmath}\n"), "");
        let p = dif_body(&o, "\\usepackage{amsmath}\n");
        for bad in [
            "\\RequirePackage",
            "\\usepackage",
            "\\new",
            "lasy",
            "\\catcode",
            "\\makeat",
            "\\DeclareOldFontCommand",
            "\\color{",
            "\\renewcommand",
        ] {
            assert!(!p.contains(bad), "{bad} in {p}");
        }
        assert!(p.contains(
            "\\providecommand{\\DIFadd}[1]{\\phitexdifmark{0 0 1}{\\phitexdifuline{#1}}} %DIF PREAMBLE\n"
        ));
        // (no XeTeX primitive named: `phitex build` of the marked-up text
        // would take it for a XeTeX document)
        assert!(!p.contains("\\XeTeX"), "{p}");
        assert!(p.contains(
            "\\providecommand{\\DIFdel}[1]{\\phitexdifmark{1 0 0}{\\phitexdifsout{#1}}} %DIF PREAMBLE\n"
        ));
        // (each definition whole on its line: braces balance per line)
        for l in p.lines() {
            let depth = l.chars().fold(0i32, |d, c| match c {
                '{' => d + 1,
                '}' => d - 1,
                _ => d,
            });
            assert_eq!(depth, 0, "{l}");
        }
        // (every \def defines a private name, but ulem's local ones while
        // it underlines)
        for (k, _) in p.match_indices("\\def\\") {
            let name = &p[k + 5..];
            assert!(
                ["phitexdif", "hfil{", "hfill{", "hss{"]
                    .iter()
                    .any(|n| name.starts_with(n)),
                "{}",
                &p[k..k + 30]
            );
        }
    }

    #[test]
    fn every_look_balances() {
        for m in Markup::ALL {
            for s in Subtype::ALL {
                let o = Options {
                    markup: m,
                    subtype: s,
                    add_color: Color::parse("#15803d").ok(),
                    ..Options::default()
                };
                for pre in ["", "\\usepackage{hyperref}\n"] {
                    let p = dif_preamble(&o, pre) + &dif_body(&o, pre);
                    for l in p.lines().filter(|l| !l.starts_with('%')) {
                        let depth = l.chars().fold(0i32, |d, c| match c {
                            '{' => d + 1,
                            '}' => d - 1,
                            _ => d,
                        });
                        if s != Subtype::OnlyChangedPage {
                            assert_eq!(depth, 0, "{m:?} {s:?}: {l}");
                        }
                    }
                    assert!(p.contains("{\\DIFadd}"), "{m:?} {s:?}");
                    assert!(p.contains("{\\DIFaddbegin}"), "{m:?} {s:?}");
                }
            }
        }
    }
}
