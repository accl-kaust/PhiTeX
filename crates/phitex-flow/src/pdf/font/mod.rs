//! The fonts of the PDF file (pdfTeX's writefont.c, writet1.c,
//! writeenc.c, mapfile.c): what the fold writes at the end for the fonts
//! the pages used, a pure function of the font files and the glyph sets.

pub mod enc;
pub mod map;
pub mod t1;
pub mod unicode;
