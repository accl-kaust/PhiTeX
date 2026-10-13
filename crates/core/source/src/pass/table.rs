//! The commands whose syntax is fixed: TeX's, e-TeX's and pdfTeX's
//! primitives, plain TeX's and LaTeX's documented commands, and the
//! environments of LaTeX and its common packages.
//!
//! A spec is a string of argument letters ([`compile`]):
//!
//! | letter | reads |
//! |---|---|
//! | `s` | an optional `*` |
//! | `o` | an optional `[…]` |
//! | `m` | an undelimited argument |
//! | `N` | one token |
//! | `c` | one character token (a relation) |
//! | `P` | parameter text up to `{` |
//! | `B` | a brace group (general text) |
//! | `b` | box specification (`to`/`spread` dimen) and a brace group |
//! | `e` | an optional `=` |
//! | `i` `d` `g` `v` | an integer, dimen, glue, or any of them |
//! | `R` | a register (`\count0`, `\parindent`) |
//! | `C` | up to `\endcsname` |
//! | `X` | an expression up to `\relax` |
//! | `f` | a file name |
//! | `T` | a token list (`{…}` or a token register) |
//! | `r` | a rule specification |
//! | `k` / `t` | the optional keyword `by` / the keyword `to` |
//! | `l` | `\let`'s target: one optional space, one token |
//! | `F` | a font's `at`/`scaled` |
//! | `K` | pdfTeX keywords, then a brace group |
//! | `Y` | `\tikz`: options, then a group or a path up to `;` |
//! | `;` | up to `;` |
//! | `V` | a verbatim run |
//! | `E` | `\begin`'s environment name and that environment's arguments |
//! | `*` | if a `*` follows, the next letter is skipped |
//! | `U`*name* | up to `\`*name* (the rest of the spec) |
//! | `A` | an array parameter: a count, then that many integers |
//! | `H` | `\parshape`: a count, then that many dimen pairs |
//! | `D` | a delimiter |
//! | `@`*name* | pdfTeX command *name*'s keyword syntax |
//! | `!`*cause* | pending, for *cause* (the rest of the spec) |
//!
//! A spec `#x` is an internal quantity of type `x` (`i`, `d`, `g`, `t`;
//! capitalized: indexed by an integer first; `c`: a constant; `F`:
//! `\fontdimen`, indexed by an integer and a font).

use std::collections::HashMap;
use std::sync::OnceLock;

use super::defs::Arg;

/// A fixed command: its arguments, or the quantity it is.
#[derive(Debug)]
pub(crate) enum Entry {
    Command(Vec<Arg>),
    /// An internal quantity: its type, and whether an integer indexes it
    /// first (`'F'`: an integer and a font).
    Quantity(char, Option<char>),
}

const INT_PARAMS: &str = "pretolerance tolerance hbadness vbadness linepenalty hyphenpenalty \
exhyphenpenalty binoppenalty relpenalty clubpenalty widowpenalty displaywidowpenalty brokenpenalty \
predisplaypenalty postdisplaypenalty interlinepenalty floatingpenalty outputpenalty \
doublehyphendemerits finalhyphendemerits adjdemerits looseness pausing holdinginserts tracingonline \
tracingmacros tracingstats tracingparagraphs tracingpages tracingoutput tracinglostchars \
tracingcommands tracingrestores tracingassigns tracinggroups tracingifs tracingscantokens \
tracingnesting language uchyph lefthyphenmin righthyphenmin globaldefs defaulthyphenchar \
defaultskewchar escapechar endlinechar newlinechar maxdeadcycles hangafter fam mag delimiterfactor \
time day month year showboxbreadth showboxdepth errorcontextlines inputlineno lastpenalty badness \
deadcycles insertpenalties spacefactor prevgraf pdfoutput pdfcompresslevel pdfobjcompresslevel \
pdfdecimaldigits pdfminorversion pdfmajorversion pdfpkresolution pdfgentounicode pdfuniqueresname \
pdfadjustspacing pdfprotrudechars pdftracingfonts pdfdraftmode pdfinclusionerrorlevel \
pdfimageresolution pdflastobj pdflastxform pdflastximage pdflastannot pdflastlink pdflastxpos \
pdflastypos pdfshellescape pdfelapsedtime pdfrandomseed lastnodetype currentgrouplevel \
currentgrouptype currentiflevel currentiftype currentifbranch eTeXversion predisplaydirection \
lastlinefit savingvdiscards savinghyphcodes TeXXeTstate interactionmode pdftexversion \
pdfforcepagebox pdfoptionalwaysusepdfpagebox pdfsuppresswarningpagegroup pdfpkfixeddpi \
pdfpagebox pdfgamma pdfimagegamma pdfimagehicolor pdfimageapplygamma pdfinfoomitdate \
pdfsuppressptexinfo pdfsuppresswarningdupdest pdfsuppresswarningdupmap pdfomitcharset \
pdfomitinfodict pdfomitprocset pdfptexuseunderscore pdfprependkern pdfappendkern \
pdfadjustinterwordglue pdfinclusioncopyfonts pdfretval pdflastximagepages pdflastximagecolordepth \
synctex pdfmovechars pdfoptionpdfinclusionerrorlevel pdfoptionpdfminorversion \
@ne tw@ thr@@ sixt@@n @cclv @cclvi @m @M @MM @xxxii m@ne @tempcnta @tempcntb count@ \
@currdepth @listdepth @enumdepth @itemdepth @lowpenalty @medpenalty @highpenalty @secpenalty";

const DIMEN_PARAMS: &str = "parindent mathsurround lineskiplimit hsize vsize maxdepth splitmaxdepth \
boxmaxdepth hfuzz vfuzz delimitershortfall nulldelimiterspace scriptspace predisplaysize \
displaywidth displayindent overfullrule hangindent hoffset voffset emergencystretch pagegoal \
pagetotal pagestretch pagefilstretch pagefillstretch pagefilllstretch pageshrink pagedepth \
prevdepth pdfpagewidth pdfpageheight pdfhorigin pdfvorigin pdflinkmargin pdfdestmargin \
pdfthreadmargin pdfpxdimen pdfeachlineheight pdfeachlinedepth pdffirstlineheight \
pdflastlinedepth pdfignoreddimen z@ p@ maxdimen @tempdima @tempdimb @tempdimc dimen@ dimen@i dimen@ii \
textwidth textheight linewidth columnwidth columnsep columnseprule marginparwidth marginparsep \
oddsidemargin evensidemargin topmargin headheight headsep footskip labelwidth labelsep leftmargin \
rightmargin itemindent listparindent fboxsep fboxrule arraycolsep tabcolsep arrayrulewidth \
doublerulesep paperwidth paperheight unitlength footnotesep @tempboxa jot mathindent";

const GLUE_PARAMS: &str = "baselineskip lineskip parskip abovedisplayskip belowdisplayskip \
abovedisplayshortskip belowdisplayshortskip leftskip rightskip topskip splittopskip tabskip \
spaceskip xspaceskip parfillskip lastskip thinmuskip medmuskip thickmuskip @tempskipa @tempskipb \
skip@ itemsep parsep topsep partopsep floatsep textfloatsep intextsep dblfloatsep dbltextfloatsep \
smallskipamount medskipamount bigskipamount @flushglue";

const TOKS_PARAMS: &str = "output everypar everymath everydisplay everyhbox everyvbox everyjob \
everycr errhelp everyeof pdfpageattr pdfpagesattr pdfpageresources pdfpkmode toks@ @temptokena";

/// Commands and their specs.
const COMMANDS: &[(&str, &str)] = &[
    // TeX's primitives
    ("relax", ""),
    ("par", ""),
    ("indent", ""),
    ("noindent", ""),
    ("hfil", ""),
    ("hfill", ""),
    ("hss", ""),
    ("hfilneg", ""),
    ("vfil", ""),
    ("vfill", ""),
    ("vss", ""),
    ("vfilneg", ""),
    ("unskip", ""),
    ("unkern", ""),
    ("unpenalty", ""),
    ("cr", ""),
    ("crcr", ""),
    ("span", ""),
    ("omit", ""),
    ("noalign", "B"),
    ("end", ""),
    ("dump", ""),
    ("begingroup", ""),
    ("endgroup", ""),
    ("bgroup", ""),
    ("egroup", ""),
    ("aftergroup", "N"),
    ("afterassignment", "N"),
    ("expandafter", "!\\expandafter"),
    ("noexpand", "N"),
    ("string", "N"),
    ("meaning", "N"),
    ("number", "i"),
    ("romannumeral", "i"),
    ("the", "R"),
    ("showthe", "R"),
    ("show", "N"),
    ("showbox", "i"),
    ("showlists", ""),
    ("showgroups", ""),
    ("showifs", ""),
    ("showtokens", "B"),
    ("message", "B"),
    ("errmessage", "B"),
    ("write", "iB"),
    ("immediate", ""),
    ("openout", "ief"),
    ("closeout", "i"),
    ("openin", "ief"),
    ("closein", "i"),
    // §1225 `read_to_cs`; etex.ch: `\readline` (defines its cs)
    ("read", "itN"),
    ("readline", "itN"),
    ("special", "B"),
    // §1101 `make_mark`; etex.ch: `\marks` a register number first
    ("mark", "B"),
    ("marks", "iB"),
    ("insert", "iB"),
    ("vadjust", "B"),
    ("uppercase", "B"),
    ("lowercase", "B"),
    ("csname", "!\\csname"),
    ("endcsname", ""),
    ("input", "f"),
    ("endinput", ""),
    ("jobname", ""),
    ("fontname", "N"),
    ("font", "NefF"),
    ("hyphenchar", "Nei"),
    ("skewchar", "Nei"),
    ("hyphenation", "B"),
    ("patterns", "B"),
    ("discretionary", "BBB"),
    ("accent", "i"),
    ("mathaccent", "i"),
    ("radical", "i"),
    ("delimiter", "i"),
    ("mathchar", "i"),
    ("char", "i"),
    // §1191 `math_left_right`, §1160 `scan_delimiter`; etex.ch: `\middle`
    ("left", "D"),
    ("right", "D"),
    ("middle", "D"),
    ("over", ""),
    ("atop", ""),
    ("above", "d"),
    // §1181 `math_fraction`: two delimiters (§1160), `\above` a dimen
    ("overwithdelims", "DD"),
    ("atopwithdelims", "DD"),
    ("abovewithdelims", "DDd"),
    ("mathchoice", "BBBB"),
    ("displaystyle", ""),
    ("textstyle", ""),
    ("scriptstyle", ""),
    ("scriptscriptstyle", ""),
    ("limits", ""),
    ("nolimits", ""),
    ("displaylimits", ""),
    ("mathord", "m"),
    ("mathop", "m"),
    ("mathbin", "m"),
    ("mathrel", "m"),
    ("mathopen", "m"),
    ("mathclose", "m"),
    ("mathpunct", "m"),
    ("mathinner", "m"),
    ("underline", "m"),
    ("overline", "m"),
    ("vcenter", "b"),
    ("hbox", "b"),
    ("vbox", "b"),
    ("vtop", "b"),
    ("box", "i"),
    ("copy", "i"),
    ("unhbox", "i"),
    ("unvbox", "i"),
    ("unhcopy", "i"),
    ("unvcopy", "i"),
    ("lastbox", ""),
    ("vsplit", "itd"),
    ("setbox", "ie"),
    ("raise", "d"),
    ("lower", "d"),
    ("moveleft", "d"),
    ("moveright", "d"),
    ("kern", "d"),
    ("mkern", "d"),
    ("hskip", "g"),
    ("vskip", "g"),
    ("mskip", "g"),
    ("hrule", "r"),
    ("vrule", "r"),
    ("penalty", "i"),
    ("leaders", ""),
    ("cleaders", ""),
    ("xleaders", ""),
    ("shipout", ""),
    ("global", ""),
    ("long", ""),
    ("outer", ""),
    ("protected", ""),
    ("def", "NPB"),
    ("gdef", "NPB"),
    ("edef", "NPB"),
    ("xdef", "NPB"),
    ("let", "Nel"),
    ("futurelet", "!\\futurelet"),
    ("chardef", "Nei"),
    ("mathchardef", "Nei"),
    ("countdef", "Nei"),
    ("dimendef", "Nei"),
    ("skipdef", "Nei"),
    ("muskipdef", "Nei"),
    ("toksdef", "Nei"),
    ("advance", "Rkv"),
    ("multiply", "Rki"),
    ("divide", "Rki"),
    ("ignorespaces", ""),
    ("ifx", "NN"),
    ("ifnum", "ici"),
    ("ifdim", "dcd"),
    ("ifodd", "i"),
    ("ifvmode", ""),
    ("ifhmode", ""),
    ("ifmmode", ""),
    ("ifinner", ""),
    ("ifvoid", "i"),
    ("ifhbox", "i"),
    ("ifvbox", "i"),
    ("ifeof", "i"),
    ("iftrue", ""),
    ("iffalse", ""),
    ("ifcase", "i"),
    ("if", "NN"),
    ("ifcat", "NN"),
    ("else", ""),
    ("fi", ""),
    ("or", ""),
    ("errorstopmode", ""),
    ("scrollmode", ""),
    ("nonstopmode", ""),
    ("batchmode", ""),
    ("nonscript", ""),
    ("noboundary", ""),
    ("nullfont", ""),
    ("topmark", ""),
    ("firstmark", ""),
    ("botmark", ""),
    ("splitfirstmark", ""),
    ("splitbotmark", ""),
    // §1248 `set_shape`: a count, then that many dimen pairs
    ("parshape", "H"),
    ("vadjust", "B"),
    // e-TeX's (etex.ch, by procedure)
    // `\protected` a prefix (§1213 `prefixed_command`), `\unless` one
    // of the conditional (`expand`, §366)
    ("unless", ""),
    // `conditional` (§498): `if_def_code` one token, `if_cs_code` up to
    // `\endcsname`, `if_font_char_code` a font and a character code
    ("ifdefined", "N"),
    ("ifcsname", "C"),
    ("iffontchar", "Ni"),
    // `pseudo_start`: general text, run as a file (may define anything)
    ("scantokens", "B"),
    // `conv_toks`/`the_toks`, `scan_general_text`: general text
    ("detokenize", "B"),
    ("unexpanded", "B"),
    ("showtokens", "B"),
    // `scan_expr`: up to `\relax`, or the first token no operand takes
    ("numexpr", "X"),
    ("dimexpr", "X"),
    ("glueexpr", "X"),
    ("muexpr", "X"),
    // `last_item` (§424): glue and mu-glue operands
    ("gluestretch", "g"),
    ("glueshrink", "g"),
    ("gluestretchorder", "g"),
    ("glueshrinkorder", "g"),
    ("mutoglue", "g"),
    ("gluetomu", "g"),
    // `fetch_box`'s kin: `\pagediscards`, `\splitdiscards` (no operand)
    ("pagediscards", ""),
    ("splitdiscards", ""),
    // `set_shape` (§1248): a count, then that many integers
    ("interlinepenalties", "A"),
    ("clubpenalties", "A"),
    ("widowpenalties", "A"),
    ("displaywidowpenalties", "A"),
    // `top_bot_mark` (§386): a register number
    ("topmarks", "i"),
    ("firstmarks", "i"),
    ("botmarks", "i"),
    ("splitfirstmarks", "i"),
    ("splitbotmarks", "i"),
    // `last_item`: `font_char_wd_code` … a font and a character code
    ("fontcharwd", "Ni"),
    ("fontcharht", "Ni"),
    ("fontchardp", "Ni"),
    ("fontcharic", "Ni"),
    // `last_item`: `par_shape_length_code` … an index
    ("parshapelength", "i"),
    ("parshapeindent", "i"),
    ("parshapedimen", "i"),
    // `last_item`: `e_TeX_version_code`, `conv_toks`: `eTeX_revision_code`
    ("eTeXrevision", ""),
    // TeX--XeT: `\beginL` … (`valign` with chr codes, no operand)
    ("beginL", ""),
    ("endL", ""),
    ("beginR", ""),
    ("endR", ""),
    // pdfTeX's (pdftex.web, "pdfTeX §N"; `@` rows: keyword syntax read
    // by `Cx::pdf`)
    ("pdfliteral", "@pdfliteral"),               // §1536
    ("pdfobj", "@pdfobj"),                       // §1542
    ("pdfannot", "@pdfannot"),                   // §1556
    ("pdfstartlink", "@pdfstartlink"),           // §1558, §1554 `scan_action`
    ("pdfendlink", ""),                          // §1559
    ("pdfdest", "@pdfdest"),                     // §1563
    ("pdfoutline", "@pdfoutline"),               // §1561
    ("pdfthread", "@pdfthread"),                 // §1565
    ("pdfstartthread", "@pdfstartthread"),       // §1566
    ("pdfendthread", ""),                        // §1567
    ("pdfxform", "@pdfxform"),                   // §1546
    ("pdfximage", "@pdfximage"),                 // §1550 `scan_image`
    ("pdfcolorstack", "@pdfcolorstack"),         // §1537
    ("pdfcolorstackinit", "@pdfcolorstackinit"), // §495
    ("pdfcatalog", "@pdfcatalog"),               // §1577
    ("pdfmdfivesum", "@pdfmdfivesum"),           // §495
    ("pdffiledump", "@pdffiledump"),             // §495
    ("pdfmatch", "@pdfmatch"),                   // §495
    ("pdffontexpand", "@pdffontexpand"),         // §703 `read_expand_font`
    // general text (`scan_pdf_ext_toks`, pdfTeX §1535)
    ("pdfinfo", "B"),
    ("pdfnames", "B"),
    ("pdftrailer", "B"),
    ("pdftrailerid", "B"),
    ("pdfsetmatrix", "B"),
    ("pdfmapfile", "B"),
    ("pdfmapline", "B"),
    ("pdfspacefont", "B"),
    ("pdfescapestring", "B"),
    ("pdfescapename", "B"),
    ("pdfescapehex", "B"),
    ("pdfunescapehex", "B"),
    ("pdffilesize", "B"),
    ("pdffilemoddate", "B"),
    ("pdfstrcmp", "BB"),
    ("pdfglyphtounicode", "BB"),
    ("expanded", "B"),
    // an object or register number (`scan_int`)
    ("pdfrefxform", "i"),
    ("pdfrefximage", "i"),
    ("pdfrefobj", "i"),
    ("pdfxformname", "i"),
    ("pdfximagebbox", "ii"),
    ("pdfpageref", "i"),
    ("pdfuniformdeviate", "i"),
    ("pdfsetrandomseed", "i"),
    ("pdfsnapycomp", "i"),
    ("pdflastmatch", "i"),
    ("pdfinsertht", "i"),
    ("leftmarginkern", "i"),
    ("rightmarginkern", "i"),
    // pdfTeX §1572 `new_snap_node`: glue
    ("pdfsnapy", "g"),
    // a font (`scan_font_ident`), then what follows
    ("pdffontattr", "NB"),
    ("pdfincludechars", "NB"),
    ("pdfnoligatures", "N"),
    ("pdffontsize", "N"),
    ("pdffontname", "N"),
    ("pdffontobjnum", "N"),
    ("pdfnobuiltintounicode", "N"),
    ("pdfcopyfont", "NN"),
    ("letterspacefont", "NNi"),
    // `\pdfprimitive` one token; `\ifpdfprimitive` one token
    ("pdfprimitive", "N"),
    ("ifpdfprimitive", "N"),
    ("ifincsname", ""),
    ("ifpdfabsnum", "ici"),
    ("ifpdfabsdim", "dcd"),
    // no operand
    ("pdfsave", ""),
    ("pdfrestore", ""),
    ("pdfsavepos", ""),
    ("pdfsnaprefpoint", ""),
    ("pdfcreationdate", ""),
    ("pdfresettimer", ""),
    ("pdfnormaldeviate", ""),
    ("pdftexrevision", ""),
    ("pdftexbanner", ""),
    ("quitvmode", ""),
    ("pdfinterwordspaceon", ""),
    ("pdfinterwordspaceoff", ""),
    ("pdffakespace", ""),
    ("pdfrunninglinkoff", ""),
    ("pdfrunninglinkon", ""),
    // plain TeX's
    ("newcount", "N"),
    ("newdimen", "N"),
    ("newskip", "N"),
    ("newmuskip", "N"),
    ("newtoks", "N"),
    ("newbox", "N"),
    ("newread", "N"),
    ("newwrite", "N"),
    ("newfam", "N"),
    ("newlanguage", "N"),
    ("newinsert", "N"),
    ("newif", "N"),
    ("newhelp", "NB"),
    ("loop", "Urepeat"),
    ("repeat", ""),
    ("line", "m"),
    ("leftline", "m"),
    ("rightline", "m"),
    ("centerline", "m"),
    ("llap", "m"),
    ("rlap", "m"),
    ("frenchspacing", ""),
    ("nonfrenchspacing", ""),
    ("bye", ""),
    ("smallskip", ""),
    ("medskip", ""),
    ("bigskip", ""),
    ("break", ""),
    ("nobreak", ""),
    ("allowbreak", ""),
    ("quad", ""),
    ("qquad", ""),
    ("enspace", ""),
    ("thinspace", ""),
    ("negthinspace", ""),
    ("enskip", ""),
    ("hglue", "g"),
    ("vglue", "g"),
    ("strut", ""),
    ("null", ""),
    ("empty", ""),
    ("space", ""),
    ("endgraf", ""),
    ("endline", ""),
    ("raggedright", ""),
    ("leavevmode", ""),
    ("narrower", ""),
    ("openup", "d"),
    ("buildrel", "!plain \\buildrel"),
    ("obeylines", ""),
    ("obeyspaces", ""),
    ("dospecials", ""),
    ("makeatletter", ""),
    ("makeatother", ""),
    ("@makeother", "N"),
    ("@sanitize", ""),
    ("catcode", "#I"),
    // LaTeX's
    ("documentclass", "omo"),
    ("usepackage", "omo"),
    ("RequirePackage", "omo"),
    ("LoadClass", "omo"),
    ("RequirePackageWithOptions", "mo"),
    ("LoadClassWithOptions", "mo"),
    ("NeedsTeXFormat", "mo"),
    ("ProvidesPackage", "mo"),
    ("ProvidesClass", "mo"),
    ("ProvidesFile", "mo"),
    ("ProvidesExplPackage", "mmmm"),
    ("ProvidesExplClass", "mmmm"),
    ("ProvidesExplFile", "mmmm"),
    ("ExplSyntaxOn", ""),
    ("ExplSyntaxOff", ""),
    ("DeclareOption", "*mm"),
    ("ExecuteOptions", "m"),
    ("ProcessOptions", "s"),
    ("PassOptionsToPackage", "mm"),
    ("PassOptionsToClass", "mm"),
    ("AtBeginDocument", "m"),
    ("AtEndDocument", "m"),
    ("AtEndOfPackage", "m"),
    ("AtEndOfClass", "m"),
    ("AtBeginDvi", "m"),
    ("begin", "E"),
    ("end", "m"),
    ("newcommand", "smoom"),
    ("renewcommand", "smoom"),
    ("providecommand", "smoom"),
    ("DeclareRobustCommand", "smoom"),
    ("newenvironment", "smoomm"),
    ("renewenvironment", "smoomm"),
    ("NewDocumentCommand", "mmm"),
    ("RenewDocumentCommand", "mmm"),
    ("ProvideDocumentCommand", "mmm"),
    ("DeclareDocumentCommand", "mmm"),
    ("NewExpandableDocumentCommand", "mmm"),
    ("NewDocumentEnvironment", "mmmm"),
    ("RenewDocumentEnvironment", "mmmm"),
    ("newtheorem", "momo"),
    ("newcounter", "mo"),
    ("setcounter", "mm"),
    ("addtocounter", "mm"),
    ("stepcounter", "m"),
    ("refstepcounter", "m"),
    ("value", "m"),
    ("arabic", "m"),
    ("roman", "m"),
    ("Roman", "m"),
    ("alph", "m"),
    ("Alph", "m"),
    ("fnsymbol", "m"),
    ("newlength", "m"),
    ("setlength", "mm"),
    ("addtolength", "mm"),
    ("settowidth", "mm"),
    ("settoheight", "mm"),
    ("settodepth", "mm"),
    ("part", "som"),
    ("chapter", "som"),
    ("section", "som"),
    ("subsection", "som"),
    ("subsubsection", "som"),
    ("paragraph", "som"),
    ("subparagraph", "som"),
    ("title", "om"),
    ("author", "om"),
    ("date", "m"),
    ("thanks", "m"),
    ("maketitle", ""),
    ("tableofcontents", ""),
    ("listoffigures", ""),
    ("listoftables", ""),
    ("label", "m"),
    ("ref", "m"),
    ("pageref", "m"),
    ("eqref", "m"),
    ("autoref", "m"),
    ("cite", "om"),
    ("nocite", "m"),
    ("bibliography", "m"),
    ("bibliographystyle", "m"),
    ("bibitem", "om"),
    ("item", "o"),
    ("caption", "om"),
    ("footnote", "om"),
    ("footnotemark", "o"),
    ("footnotetext", "om"),
    ("marginpar", "om"),
    ("textbf", "m"),
    ("textit", "m"),
    ("textsl", "m"),
    ("textsc", "m"),
    ("texttt", "m"),
    ("textrm", "m"),
    ("textsf", "m"),
    ("textup", "m"),
    ("textmd", "m"),
    ("textnormal", "m"),
    ("emph", "m"),
    ("mbox", "m"),
    ("fbox", "m"),
    ("makebox", "oom"),
    ("framebox", "oom"),
    ("parbox", "ooomm"),
    ("raisebox", "moom"),
    ("rule", "omm"),
    ("hspace", "sm"),
    ("vspace", "sm"),
    ("addvspace", "m"),
    ("addpenalty", "i"),
    ("newpage", ""),
    ("clearpage", ""),
    ("cleardoublepage", ""),
    ("pagebreak", "o"),
    ("nopagebreak", "o"),
    ("linebreak", "o"),
    ("nolinebreak", "o"),
    ("\\", "so"),
    ("newline", ""),
    ("centering", ""),
    ("raggedleft", ""),
    ("tiny", ""),
    ("scriptsize", ""),
    ("footnotesize", ""),
    ("small", ""),
    ("normalsize", ""),
    ("large", ""),
    ("Large", ""),
    ("LARGE", ""),
    ("huge", ""),
    ("Huge", ""),
    ("bfseries", ""),
    ("itshape", ""),
    ("mdseries", ""),
    ("upshape", ""),
    ("slshape", ""),
    ("scshape", ""),
    ("rmfamily", ""),
    ("sffamily", ""),
    ("ttfamily", ""),
    ("normalfont", ""),
    ("em", ""),
    ("selectfont", ""),
    ("fontsize", "mm"),
    ("fontfamily", "m"),
    ("fontseries", "m"),
    ("fontshape", "m"),
    ("fontencoding", "m"),
    ("usefont", "mmmm"),
    ("DeclareFontFamily", "mmm"),
    ("DeclareFontShape", "mmmmmm"),
    ("ensuremath", "m"),
    ("mathrm", "m"),
    ("mathbf", "m"),
    ("mathit", "m"),
    ("mathsf", "m"),
    ("mathtt", "m"),
    ("mathcal", "m"),
    ("mathnormal", "m"),
    ("frac", "mm"),
    ("sqrt", "om"),
    ("text", "m"),
    ("include", "m"),
    ("includeonly", "m"),
    ("InputIfFileExists", "mmm"),
    ("IfFileExists", "mmm"),
    ("@ifundefined", "mmm"),
    ("@ifpackageloaded", "mmm"),
    ("@ifclassloaded", "mmm"),
    ("@ifpackagewith", "mmmm"),
    ("@ifnextchar", "!lookahead"),
    ("@ifstar", "!lookahead"),
    ("kernel@ifnextchar", "!lookahead"),
    ("new@ifnextchar", "!lookahead"),
    ("@ifnch", "!lookahead"),
    ("@firstofone", "m"),
    ("@firstoftwo", "mm"),
    ("@secondoftwo", "mm"),
    ("@gobble", "m"),
    ("@gobbletwo", "mm"),
    ("@gobblefour", "mmmm"),
    ("@namedef", "mPB"),
    ("@nameuse", "!\\csname"),
    ("@ifdefinable", "Nm"),
    ("protected@edef", "NPB"),
    ("protected@xdef", "NPB"),
    ("g@addto@macro", "Nm"),
    ("@onlypreamble", "N"),
    ("protect", ""),
    ("pagestyle", "m"),
    ("thispagestyle", "m"),
    ("pagenumbering", "m"),
    ("twocolumn", "o"),
    ("onecolumn", ""),
    ("appendix", ""),
    ("hline", ""),
    ("cline", "m"),
    ("multicolumn", "mmm"),
    ("tabularnewline", ""),
    ("vline", ""),
    ("color", "om"),
    ("textcolor", "omm"),
    ("definecolor", "ommm"),
    ("colorbox", "omm"),
    ("includegraphics", "som"),
    ("graphicspath", "m"),
    ("href", "Vm"),
    ("url", "V"),
    ("nolinkurl", "V"),
    ("verb", "V"),
    ("Verb", "V"),
    ("lstinline", "V"),
    ("mintinline", "V"),
    ("index", "m"),
    ("glossary", "m"),
    ("hyperref", "om"),
    ("hypersetup", "m"),
    ("pdfbookmark", "omm"),
    ("phantomsection", ""),
    ("DeclareMathOperator", "smm"),
    ("operatorname", "sm"),
    ("boldsymbol", "m"),
    ("mathbb", "m"),
    ("mathfrak", "m"),
    ("hat", "m"),
    ("bar", "m"),
    ("tilde", "m"),
    ("vec", "m"),
    ("dot", "m"),
    ("ddot", "m"),
    ("check", "m"),
    ("breve", "m"),
    ("acute", "m"),
    ("grave", "m"),
    ("widehat", "m"),
    ("widetilde", "m"),
    ("overbrace", "m"),
    ("underbrace", "m"),
    ("stackrel", "mm"),
    ("overset", "mm"),
    ("underset", "mm"),
    ("binom", "mm"),
    ("dfrac", "mm"),
    ("tfrac", "mm"),
    ("big", "N"),
    ("Big", "N"),
    ("bigg", "N"),
    ("Bigg", "N"),
    ("bigl", "N"),
    ("bigr", "N"),
    ("Bigl", "N"),
    ("Bigr", "N"),
    ("biggl", "N"),
    ("biggr", "N"),
    ("cdots", ""),
    ("ldots", ""),
    ("dots", ""),
    ("tikzset", "m"),
    ("usetikzlibrary", "m"),
    ("pgfkeys", "m"),
    ("pgfqkeys", "mm"),
    ("pgfkeysvalueof", "m"),
    ("pgfmathsetmacro", "mm"),
    ("pgfmathparse", "m"),
    ("pgfmathsetlengthmacro", "mm"),
    ("pgfmathtruncatemacro", "mm"),
    ("draw", ";"),
    ("fill", ";"),
    ("filldraw", ";"),
    ("path", ";"),
    ("node", ";"),
    ("coordinate", ";"),
    ("shade", ";"),
    ("shadedraw", ";"),
    ("clip", ";"),
    ("matrix", ";"),
    ("pic", ";"),
    ("useasboundingbox", ";"),
    ("tikz", "Y"),
    ("foreach", "!\\foreach list syntax"),
];

/// Environments and the arguments after `\begin{name}`.
const ENVIRONMENTS: &[(&str, &str)] = &[
    ("document", ""),
    ("figure", "o"),
    ("figure*", "o"),
    ("table", "o"),
    ("table*", "o"),
    ("tabular", "om"),
    ("tabular*", "mom"),
    ("tabularx", "mom"),
    ("array", "om"),
    ("minipage", "ooom"),
    ("itemize", "o"),
    ("enumerate", "o"),
    ("description", "o"),
    ("center", ""),
    ("flushleft", ""),
    ("flushright", ""),
    ("quote", ""),
    ("quotation", ""),
    ("verse", ""),
    ("abstract", ""),
    ("titlepage", ""),
    ("equation", ""),
    ("equation*", ""),
    ("align", ""),
    ("align*", ""),
    ("gather", ""),
    ("gather*", ""),
    ("multline", ""),
    ("multline*", ""),
    ("flalign", ""),
    ("flalign*", ""),
    ("eqnarray", ""),
    ("eqnarray*", ""),
    ("displaymath", ""),
    ("math", ""),
    ("alignat", "m"),
    ("alignat*", "m"),
    ("split", ""),
    ("cases", ""),
    ("matrix", ""),
    ("pmatrix", ""),
    ("bmatrix", ""),
    ("vmatrix", ""),
    ("Vmatrix", ""),
    ("smallmatrix", ""),
    ("list", "mm"),
    ("trivlist", ""),
    ("thebibliography", "m"),
    ("proof", "o"),
    ("tikzpicture", "o"),
    ("scope", "o"),
    ("pgfpicture", ""),
    ("pgfscope", ""),
    ("letter", "m"),
    ("filecontents", ""),
    ("filecontents*", ""),
    ("verbatim", ""),
    ("verbatim*", ""),
    ("Verbatim", ""),
    ("lstlisting", ""),
    ("minted", ""),
    ("comment", ""),
    ("picture", "!picture coordinate syntax"),
];

pub(crate) struct Table {
    pub commands: HashMap<&'static str, Entry>,
    pub envs: HashMap<&'static str, Vec<Arg>>,
    /// The names that are TeX's, e-TeX's or pdfTeX's primitives (the
    /// rest of `commands` are the formats' macros, whose bodies the
    /// table does not give).
    pub primitives: std::collections::HashSet<&'static str>,
}

impl Table {
    /// Whether `name` is a primitive of TeX, e-TeX or pdfTeX.
    pub(crate) fn primitive(&self, name: &str) -> bool {
        self.primitives.contains(name)
    }
}

/// The table, built once.
pub(crate) fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        let mut commands = HashMap::new();
        for (list, ty) in [
            (INT_PARAMS, 'i'),
            (DIMEN_PARAMS, 'd'),
            (GLUE_PARAMS, 'g'),
            (TOKS_PARAMS, 't'),
        ] {
            for n in list.split_whitespace() {
                commands.insert(n, Entry::Quantity(ty, None));
            }
        }
        for (n, ty, ix) in [
            ("count", 'i', 'i'),
            ("dimen", 'd', 'i'),
            ("skip", 'g', 'i'),
            ("muskip", 'g', 'i'),
            ("toks", 't', 'i'),
            ("catcode", 'i', 'i'),
            ("lccode", 'i', 'i'),
            ("uccode", 'i', 'i'),
            ("sfcode", 'i', 'i'),
            ("mathcode", 'i', 'i'),
            ("delcode", 'i', 'i'),
            ("wd", 'd', 'i'),
            ("ht", 'd', 'i'),
            ("dp", 'd', 'i'),
            ("fontdimen", 'd', 'F'),
            ("lpcode", 'i', 'F'),
            ("rpcode", 'i', 'F'),
            ("efcode", 'i', 'F'),
        ] {
            commands.insert(n, Entry::Quantity(ty, Some(ix)));
        }
        // (the quantities above are primitives; the rows of `COMMANDS`
        // are, up to plain TeX's first)
        let mut primitives: std::collections::HashSet<&'static str> =
            commands.keys().copied().collect();
        for &(n, spec) in COMMANDS {
            if n == "newcount" {
                break;
            }
            primitives.insert(n);
            let _ = spec;
        }
        for &(n, spec) in COMMANDS {
            if !commands.contains_key(n) {
                commands.insert(n, Entry::Command(compile(spec)));
            }
        }
        let envs = ENVIRONMENTS.iter().map(|&(n, s)| (n, compile(s))).collect();
        Table {
            commands,
            envs,
            primitives,
        }
    })
}

/// A spec's arguments.
pub(crate) fn compile(spec: &str) -> Vec<Arg> {
    let mut out = Vec::new();
    for (k, c) in spec.char_indices() {
        out.push(match c {
            's' => Arg::Star,
            'o' => Arg::Opt,
            'm' => Arg::M,
            'N' => Arg::Tok,
            'c' => Arg::Relation,
            'P' => Arg::ParamText,
            'B' => Arg::Group,
            'b' => Arg::BoxSpec,
            'e' => Arg::Eq,
            'i' => Arg::Int,
            'd' => Arg::Dimen,
            'g' => Arg::Glue,
            'v' => Arg::Value,
            'R' => Arg::Reg,
            'C' => Arg::CsName,
            'X' => Arg::Expr,
            'f' => Arg::File,
            'T' => Arg::Toks,
            'r' => Arg::Rule,
            'k' => Arg::By,
            't' => Arg::To,
            'l' => Arg::LetTarget,
            'F' => Arg::FontSize,
            'K' => Arg::Keywords,
            'A' => Arg::Array('i'),
            'H' => Arg::Array('P'),
            'D' => Arg::Delim,
            'Y' => Arg::Tikz,
            ';' => Arg::Semicolon,
            'V' => Arg::Verb,
            'E' => Arg::Env,
            '*' => Arg::StarSkip,
            'U' => {
                out.push(Arg::UntilCs(spec[k + 1..].to_owned()));
                return out;
            }
            '!' => {
                out.push(Arg::Pending(spec[k + 1..].to_owned()));
                return out;
            }
            '@' => {
                out.push(Arg::Pdf(spec[k + 1..].to_owned()));
                return out;
            }
            _ => unreachable!("spec letter {c}"),
        });
    }
    out
}
