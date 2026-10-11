//! Knuth's typesetting algorithms as pure functions: values in, values out.
//!
//! Scaled arithmetic and glue (tex.web §99–§110), hpack and vpack
//! (§644–§679), line breaking (§813–§890), hyphenation (§891–§965), math
//! layout (§699–§767), alignment (§768–§812), the page builder's decisions
//! (§980–§1028).
//!
//! Rule: no dependency on `phitex-reduce` or `phitex-tex`. A function here
//! is given every value it needs; whatever must be asked of the document
//! (a register, a macro) is asked by `phitex-tex`, which then calls in.
//! Shared by every engine; each function carries its oracle tests per
//! engine.
