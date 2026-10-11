//! LuaTeX on `phitex-tex`: UTF-8 input, `\directlua` and the callbacks,
//! the `node`, `tex`, `token` and `font` libraries on `phitex-lua`,
//! OpenType fonts (`phitex-otf`), and LuaTeX's PDF backend. LuaLaTeX is
//! this engine with the LaTeX format.
//!
//! A Lua call is one step to the reduction core: what it read and wrote in
//! its last run decides whether it runs again.
