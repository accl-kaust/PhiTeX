#![allow(clippy::pedantic)]

mod toy;

use toy::*;

#[test]
fn a_document_builds() {
    let src = r"\def\x{hello world} \x \step \step \the\count \par
        { \def\x{inner} \x } \x \write{out1} \message{log1} \par
        \ifzero\y yes \else no \fi \label{k} \ref{k} \hbox{a b \the\count} \par";
    let mut d = Doc::new(ids(lex(src)));
    d.g.cfg.check = true;
    let r = d.g.run();
    eprintln!("{r:?}\n{}", d.observe());
    assert!(r.iterations >= 1);
}
