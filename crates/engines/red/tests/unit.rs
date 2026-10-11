//! Unit-level: one construct per test, observed through `\errmessage`.
mod common;
use common::*;

fn one(src: &str) -> String {
    let v = errs(src);
    assert_eq!(v.len(), 1, "diags: {v:?}");
    v[0].clone()
}

// ---- tokenizer
#[test]
fn tokenizer_catcodes_and_spaces() {
    assert_eq!(one("\\errmessage{a  b\\relax c}"), "a b\\relax c");
    assert_eq!(one("\\catcode`\\Q=11 \\def\\aQb{X}\\errmessage{\\aQb}"), "X");
    assert_eq!(one("\\errmessage{^^41^^5a}"), "AZ");
    assert_eq!(one("\\errmessage{x% comment\ny}"), "xy");
}

#[test]
fn tokenizer_endlinechar_and_par() {
    assert_eq!(one("\\def\\p{\\par}\\def\\x#1\\par{[#1]}\\errmessage{\\x ab\n\nc}"), "[ab] c");
    assert_eq!(one("\\endlinechar=-1 \\errmessage{a\nb}"), "ab");
}

// ---- macros
#[test]
fn macros_args() {
    assert_eq!(one("\\def\\a#1#2{(#2,#1)}\\errmessage{\\a xy\\a{ab}{cd}}"), "(y,x)(cd,ab)");
    assert_eq!(one("\\def\\b#1.#2\\stop{[#1|#2]}\\errmessage{\\b {x}.{y}\\stop\\b {x}y.z\\stop}"), "[x|y][{x}y|z]");
    assert_eq!(one("\\def\\c#1#{<#1>}\\errmessage{\\c abc{d}}"), "<abc>{d}");
    assert_eq!(one("\\def\\a#1{\\def\\b##1{#1##1}}\\a x\\errmessage{\\b y}"), "xy");
}

#[test]
fn macros_meaning() {
    assert_eq!(one("\\long\\def\\a#1#2.{##1}\\errmessage{\\meaning\\a}"), "\\long macro:#1#2.->##1");
}

#[test]
fn macro_recursion_terminates() {
    assert_eq!(
        one("\\count1=0 \\def\\l{\\advance\\count1 1 \\ifnum\\count1<500 \\expandafter\\l\\fi}\\l\\errmessage{\\the\\count1}"),
        "500"
    );
}

// ---- conditionals as select
#[test]
fn conditionals() {
    assert_eq!(one("\\errmessage{\\ifnum1<2 a\\else b\\fi\\ifx aa c\\else d\\fi\\if ab e\\else f\\fi}"), "acf");
    assert_eq!(one("\\errmessage{\\ifcase2 a\\or b\\or c\\else d\\fi\\ifcase9 a\\or b\\else z\\fi}"), "cz");
    // untaken arm is skipped raw: undefined commands and unbalanced \if inside are fine
    assert_eq!(one("\\errmessage{\\iffalse \\undefined\\ifnum\\else\\fi x\\else y\\fi}"), "y");
    assert_eq!(one("\\def\\t{\\iftrue}\\errmessage{\\t a\\else b\\fi}"), "a");
    assert_eq!(one("\\errmessage{\\ifdim1pt<1.5pt a\\fi\\ifodd3 b\\fi\\ifvmode c\\fi}"), "abc");
}

#[test]
fn conditional_nesting() {
    assert_eq!(one("\\errmessage{\\ifnum1=1 \\ifnum2=3 a\\else b\\fi\\else c\\fi}"), "b");
}

// ---- groups
#[test]
fn groups_restore() {
    assert_eq!(one("\\count1=1 {\\count1=2 }\\errmessage{\\the\\count1}"), "1");
    assert_eq!(one("\\count1=1 {\\global\\count1=2 }\\errmessage{\\the\\count1}"), "2");
    assert_eq!(one("\\begingroup\\def\\a{x}\\endgroup\\errmessage{\\meaning\\a}"), "undefined");
    assert_eq!(one("\\count1=1 {\\count1=2 {\\global\\count1=3 }\\count1=4 }\\errmessage{\\the\\count1}"), "3");
}

#[test]
fn aftergroup_order() {
    assert_eq!(one("\\def\\m#1#2{\\errmessage{#1#2}}{\\aftergroup\\m\\aftergroup x\\aftergroup y}"), "xy");
}

// ---- registers and arithmetic
#[test]
fn registers() {
    assert_eq!(one("\\count3=7 \\multiply\\count3 by 6 \\advance\\count3 -2 \\divide\\count3 by 4 \\errmessage{\\the\\count3}"), "10");
    assert_eq!(one("\\dimen0=1.5pt \\multiply\\dimen0 3 \\errmessage{\\the\\dimen0}"), "4.5pt");
    assert_eq!(one("\\skip0=1pt plus 2fil minus 3fill \\errmessage{\\the\\skip0}"), "1.0pt plus 2.0fil minus 3.0fill");
    assert_eq!(one("\\dimen0=1in \\errmessage{\\the\\dimen0}"), "72.26999pt");
    assert_eq!(one("\\count1=-7 \\divide\\count1 2 \\errmessage{\\the\\count1}"), "-3");
    assert_eq!(one("\\toks0={a#b}\\errmessage{\\the\\toks0}"), "a##b");
    assert_eq!(one("\\countdef\\c=5 \\c=9 \\errmessage{\\the\\count5}"), "9");
}

#[test]
fn number_romannumeral() {
    assert_eq!(one("\\errmessage{\\number-0012 \\romannumeral1984 \\number`\\a}"), "-12 mcmlxxxiv97");
}

#[test]
fn etex_expr() {
    let v = msgs(&run_etex("\\errmessage{\\the\\numexpr7*(3+4)/2\\relax\\the\\dimexpr1pt*3/4\\relax}"));
    assert_eq!(v, ["250.75pt"]);
}

// ---- \edef, \let, \futurelet, \expandafter, \noexpand
#[test]
fn edef_and_friends() {
    assert_eq!(one("\\def\\a{x}\\edef\\b{\\a\\noexpand\\a}\\def\\a{y}\\errmessage{\\meaning\\b}"), "macro:->x\\a ");
    assert_eq!(one("\\def\\a{x}\\let\\b=\\a\\def\\a{y}\\errmessage{\\b\\a}"), "xy");
    assert_eq!(one("\\def\\s#1{\\errmessage{\\meaning\\n}}\\futurelet\\n\\s Q"), "the letter Q");
    assert_eq!(one("\\def\\a{b}\\def\\x#1{[#1]}\\errmessage{\\expandafter\\x\\a}"), "[b]");
    assert_eq!(one("\\toks0={u}\\edef\\b{\\the\\toks0 v}\\errmessage{\\b}"), "uv");
}

// ---- \csname
#[test]
fn csname() {
    assert_eq!(one("\\expandafter\\def\\csname a b\\endcsname{Q}\\errmessage{\\csname a b\\endcsname}"), "Q");
    assert_eq!(one("\\errmessage{\\expandafter\\meaning\\csname zz\\endcsname}"), "\\relax");
    assert_eq!(one("\\def\\n{7}\\expandafter\\def\\csname x\\n\\endcsname{S}\\errmessage{\\csname x7\\endcsname\\string\\x}"), "S\\x");
}

// ---- boxes, alignments, math, line breaking (dimensions observed via \the)
const FONT: &str = "\\font\\r=cmr10 \\r \\hsize=100pt \\parindent=0pt \\baselineskip=12pt \\tolerance=10000 \\parfillskip=0pt plus 1fil ";

#[test]
fn hbox_width() {
    assert_eq!(one(&format!("{FONT}\\setbox0\\hbox{{\\kern3pt\\vrule width2pt\\hskip4pt}}\\errmessage{{\\the\\wd0}}")), "9.0pt");
    assert_eq!(one(&format!("{FONT}\\setbox0\\hbox to20pt{{\\hfil}}\\errmessage{{\\the\\wd0}}")), "20.0pt");
    assert_eq!(one(&format!("{FONT}\\setbox0\\hbox{{A}}\\errmessage{{\\the\\wd0}}")), "7.50002pt");
}

#[test]
fn vbox_height() {
    assert_eq!(
        one(&format!("{FONT}\\setbox0\\vbox{{\\hrule height1pt\\vskip2pt\\hrule height3pt depth1pt}}\\errmessage{{\\the\\ht0+\\the\\dp0}}")),
        "6.0pt+1.0pt"
    );
}

#[test]
fn alignment() {
    assert_eq!(
        one(&format!("{FONT}\\setbox0\\vbox{{\\halign{{#\\tabskip2pt&\\hfil#\\cr\\vrule width5pt&\\vrule width1pt\\cr\\vrule width1pt&\\vrule width7pt\\cr}}}}\\errmessage{{\\the\\wd0}}")),
        "14.0pt"
    );
}

#[test]
fn math() {
    let s = one(&format!("\\textfont0=\\r \\scriptfont0=\\r \\scriptscriptfont0=\\r \\font\\s=cmsy10 \\textfont2=\\s \\scriptfont2=\\s \\scriptscriptfont2=\\s \\font\\e=cmex10 \\textfont3=\\e \\scriptfont3=\\e \\scriptscriptfont3=\\e {FONT}\\setbox0\\hbox{{$\\mathchar\"0141$}}\\errmessage{{\\the\\wd0}}"));
    assert_eq!(s, "7.50002pt");
}

#[test]
fn line_breaking() {
    let words = "word ".repeat(30);
    assert_eq!(
        one(&format!("{FONT}\\setbox0\\vbox{{{words}\\par\\global\\count1=\\prevgraf}}\\errmessage{{\\the\\count1}}")).parse::<i32>().unwrap() > 1,
        true
    );
    assert_eq!(one(&format!("{FONT}\\setbox0\\vbox{{\\hsize=50pt A\\par}}\\errmessage{{\\the\\wd0}}")), "50.0pt");
}
