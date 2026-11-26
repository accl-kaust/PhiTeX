//! A contact sheet: several pages drawn in one picture, a grid of them,
//! each under its page number (`phitex session png --changed`, `--page
//! N-M`, `--at` a line of overlays): one PNG an agent reads at a glance
//! instead of a dozen.

/// The most pages a sheet holds; more are said ("+N more"), not drawn.
pub const MAX: usize = 12;

/// The width a page is drawn at on a sheet, in pixels: its text readable.
pub const CELL_W: f64 = 640.0;

/// A 5×7 glyph for each character the labels use (`p. 0-9`), a row a
/// byte, its five low bits the pixels from the left.
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'p' => [0x00, 0x00, 0x1E, 0x11, 0x1E, 0x10, 0x10],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        _ => [0; 7],
    }
}

/// RGBA pixels, `w` wide.
struct Canvas {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32, grey: u8) -> Canvas {
        Canvas {
            w,
            h,
            px: [grey, grey, grey, 255].repeat(w as usize * h as usize),
        }
    }

    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, rgb: [u8; 3]) {
        for yy in y..(y + h).min(self.h) {
            for xx in x..(x + w).min(self.w) {
                let k = (yy as usize * self.w as usize + xx as usize) * 4;
                self.px[k..k + 3].copy_from_slice(&rgb);
            }
        }
    }

    fn blit(&mut self, x: u32, y: u32, s: &phitex_render::Strip) {
        for yy in 0..s.h.min(self.h.saturating_sub(y)) {
            let from = yy as usize * s.w as usize * 4;
            let n = s.w.min(self.w.saturating_sub(x)) as usize * 4;
            let to = ((y + yy) as usize * self.w as usize + x as usize) * 4;
            self.px[to..to + n].copy_from_slice(&s.rgba[from..from + n]);
        }
    }

    /// `text` at (x, y), each glyph pixel `size` pixels a side.
    fn text(&mut self, x: u32, y: u32, text: &str, size: u32, rgb: [u8; 3]) {
        for (i, c) in text.chars().enumerate() {
            let g = glyph(c);
            let gx = x + u32::try_from(i).unwrap_or(0) * 6 * size;
            for (row, bits) in g.iter().enumerate() {
                for col in 0..5u32 {
                    if bits & (0x10 >> col) != 0 {
                        let row = u32::try_from(row).unwrap_or(0);
                        self.fill(gx + col * size, y + row * size, size, size, rgb);
                    }
                }
            }
        }
    }
}

/// The columns of a sheet of `n` pages.
fn columns(n: usize) -> usize {
    match n {
        0..=3 => n.max(1),
        4 => 2,
        5..=9 => 3,
        _ => 4,
    }
}

/// The sheet of `pages` (each its number from 1 and its drawing): a
/// grid, each page under "p. N", on grey. Its width, height and RGBA.
pub fn sheet(pages: &[(usize, phitex_render::Strip)]) -> (u32, u32, Vec<u8>) {
    let gap = 16u32;
    let label = 44u32;
    let cols = columns(pages.len());
    let rows = pages.len().div_ceil(cols);
    let cw = pages.iter().map(|(_, s)| s.w).max().unwrap_or(1);
    let ch = pages.iter().map(|(_, s)| s.h).max().unwrap_or(1);
    let cols32 = u32::try_from(cols).unwrap_or(1);
    let rows32 = u32::try_from(rows).unwrap_or(1);
    let w = gap + cols32 * (cw + gap);
    let h = gap + rows32 * (label + ch + gap);
    let mut c = Canvas::new(w, h, 224);
    for (i, (page, s)) in pages.iter().enumerate() {
        let (col, row) = (
            u32::try_from(i % cols).unwrap_or(0),
            u32::try_from(i / cols).unwrap_or(0),
        );
        let x = gap + col * (cw + gap);
        let y = gap + row * (label + ch + gap);
        c.text(x, y + 6, &format!("p.{page}"), 4, [20, 20, 20]);
        // (a frame around the page, so a white page shows its edge)
        c.fill(
            x.saturating_sub(1),
            y + label - 1,
            s.w + 2,
            s.h + 2,
            [120, 120, 120],
        );
        c.blit(x, y + label, s);
    }
    (c.w, c.h, c.px)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(w: u32, h: u32) -> phitex_render::Strip {
        phitex_render::Strip {
            w,
            h,
            rgba: [255, 255, 255, 255].repeat(w as usize * h as usize),
        }
    }

    /// Twelve pages make four columns of three rows; each cell's label is
    /// drawn above its page.
    #[test]
    fn a_grid_of_pages() {
        let pages: Vec<_> = (1..=12).map(|k| (k, page(100, 80))).collect();
        let (w, h, px) = sheet(&pages);
        assert_eq!((w, h), (16 + 4 * 116, 16 + 3 * (44 + 80 + 16)));
        assert_eq!(px.len(), (w * h * 4) as usize);
        // (the first label's "p" is dark, its page white)
        let at = |x: u32, y: u32| px[((y * w + x) * 4) as usize];
        assert!((0..30).any(|dx| (0..28).any(|dy| at(16 + dx, 22 + dy) < 100)));
        assert_eq!(at(16 + 50, 16 + 44 + 40), 255);
        assert_eq!(columns(1), 1);
        assert_eq!(columns(3), 3);
        assert_eq!(columns(5), 3);
    }
}
