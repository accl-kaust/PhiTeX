//! GFtoPK: a generic font file (GF, METAFONT's output) packed into a PK
//! file, as TeX Live's `gftopk` does it: gftopk.web (version 2.4) with
//! web2c's gftopk.ch.
//!
//! The change file's differences from the WEB that show in the output are
//! kept: the PK preamble's comment is the GF preamble's comment as it is
//! (leading blanks removed), not `GFtoPK 2.4 output from ` and it (§81,
//! §83: `comm_length` and `from_length` are 0), and the row array grows
//! by 16000 counts when it fills (§48, §51, §56) where the WEB gave up.
//! The terminal output, which gftopk prints only with `-v`, is not made;
//! the messages of `abort` and `bad_gf` (gftopk writes them to stderr and
//! exits with status 1) are the errors.
//!
//! Section numbers (§) are gftopk.web's.

#![allow(
    clippy::unreadable_literal,
    clippy::many_single_char_names,
    clippy::too_many_lines,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::similar_names,
    clippy::if_not_else,
    clippy::doc_markdown
)]

/// `GFtoPK`'s identification for the comment (§1).
pub const BANNER: &str = "This is GFtoPK, Version 2.4";

// GF opcodes (§18)
const PAINT_0: i32 = 0;
const PAINT1: i32 = 64;
const BOC: i32 = 67;
const BOC1: i32 = 68;
const EOC: i32 = 69;
const SKIP0: i32 = 70;
const NEW_ROW_0: i32 = 74;
const MAX_NEW_ROW: i32 = 238;
const XXX1: i32 = 239;
const YYY: i32 = 243;
const NO_OP: i32 = 244;
const CHAR_LOC: i32 = 245;
const CHAR_LOC0: i32 = 246;
const PRE: i32 = 247;
const POST: i32 = 248;
const POST_POST: i32 = 249;
const GF_ID_BYTE: i32 = 131;

// PK opcodes (§24)
const PK_ID: i32 = 89;
const PK_XXX1: i32 = 240;
const PK_YYY: i32 = 244;
const PK_POST: i32 = 245;
const PK_NO_OP: i32 = 246;
const PK_PRE: i32 = 247;

// the character status (§48)
const VIRGIN: u8 = 0;
const LOCATED: u8 = 1;
const SENT: u8 = 2;

// the row array's markers (§50)
const END_OF_ROW: i32 = -99999;
const END_OF_CHAR: i32 = -99998;

/// The row array's first size and its step when it grows (§6, gftopk.ch).
const MAX_ROW_STEP: usize = 16000;

/// The PK file of GF file `gf`, byte for byte TeX Live's `gftopk`'s.
///
/// # Errors
///
/// The message gftopk prints before it stops when the GF file is
/// malformed (`Bad GF file: ...!`, §8).
pub fn gf_to_pk(gf: &[u8]) -> Result<Vec<u8>, String> {
    let mut g = GfToPk::new(gf);
    g.convert_gf_file()?;
    Ok(g.pk)
}

struct GfToPk<'a> {
    gf: &'a [u8],
    /// where the next byte is read (`gf_loc`, `set_pos`, §45)
    gf_pos: usize,
    gf_len: i32,
    pk: Vec<u8>,
    pk_open: bool,
    bit_weight: i32,
    output_byte: i32,
    tfm_width: [i32; 256],
    dx: [i32; 256],
    dy: [i32; 256],
    status: [u8; 256],
    row: Vec<i32>,
    max_row: usize,
    gf_ch: i32,
    gf_ch_mod_256: i32,
    pred_pk_loc: i32,
    max_n: i32,
    min_n: i32,
    max_m: i32,
    min_m: i32,
    row_ptr: i32,
    power: [i32; 9],
    check_sum: i32,
    design_size: i32,
    hppp: i32,
    vppp: i32,
}

fn bad_gf(what: &str) -> String {
    format!("Bad GF file: {what}!")
}

impl<'a> GfToPk<'a> {
    fn new(gf: &'a [u8]) -> Self {
        let mut power = [0; 9];
        power[0] = 1;
        for i in 1..9 {
            power[i] = power[i - 1] + power[i - 1];
        }
        GfToPk {
            gf,
            gf_pos: 0,
            gf_len: 0,
            pk: Vec::new(),
            pk_open: false,
            bit_weight: 0,
            output_byte: 0,
            tfm_width: [0; 256],
            dx: [0; 256],
            dy: [0; 256],
            status: [VIRGIN; 256],
            row: vec![0; MAX_ROW_STEP + 1],
            max_row: MAX_ROW_STEP,
            gf_ch: 0,
            gf_ch_mod_256: 0,
            pred_pk_loc: 0,
            max_n: 0,
            min_n: 0,
            max_m: 0,
            min_m: 0,
            row_ptr: 0,
            power,
            check_sum: 0,
            design_size: 0,
            hppp: 0,
            vppp: 0,
        }
    }

    // §43: the next byte, unsigned
    fn gf_byte(&mut self) -> Result<i32, String> {
        match self.gf.get(self.gf_pos) {
            Some(&b) => {
                self.gf_pos += 1;
                Ok(i32::from(b))
            }
            None => Err(bad_gf("Unexpected end of file!")),
        }
    }

    // §43: the next four bytes, signed
    fn gf_signed_quad(&mut self) -> Result<i32, String> {
        let mut a = 0u32;
        for _ in 0..4 {
            a = (a << 8) | self.gf_byte()? as u32;
        }
        Ok(a as i32)
    }

    /// `pk_loc` (§41): the number of bytes written
    fn pk_loc(&self) -> i32 {
        self.pk.len() as i32
    }

    // §44 (gftopk.ch: `putbyte`, the low eight bits)
    fn pk_byte(&mut self, a: i32) {
        if self.pk_open {
            self.pk.push(a as u8);
        }
    }

    fn pk_halfword(&mut self, mut a: i32) {
        if a < 0 {
            a += 65536;
        }
        self.pk.push((a / 256) as u8);
        self.pk.push((a % 256) as u8);
    }

    fn pk_three_bytes(&mut self, a: i32) {
        self.pk.push((a / 65536 % 256) as u8);
        self.pk.push((a / 256 % 256) as u8);
        self.pk.push((a % 256) as u8);
    }

    // (two's complement: the WEB's arithmetic gives these bytes)
    fn pk_word(&mut self, a: i32) {
        if self.pk_open {
            self.pk.extend_from_slice(&a.to_be_bytes());
        }
    }

    fn pk_nyb(&mut self, a: i32) {
        if self.bit_weight == 16 {
            self.output_byte = a * 16;
            self.bit_weight = 1;
        } else {
            self.pk_byte(self.output_byte + a);
            self.bit_weight = 16;
        }
    }

    // §46
    fn move_to_byte(&mut self, n: i32) {
        self.gf_pos = usize::try_from(n).unwrap_or(usize::MAX);
    }

    fn row(&self, i: i32) -> i32 {
        self.row[i as usize]
    }

    fn set_row(&mut self, i: i32, v: i32) {
        self.row[i as usize] = v;
    }

    // §56 (gftopk.ch: the array grows instead of failing)
    fn put_in_rows(&mut self, v: i32) {
        if self.row_ptr as usize > self.max_row {
            let new_row = self.max_row + MAX_ROW_STEP;
            self.row.resize(new_row + 1, 0);
            self.max_row = new_row;
        }
        let p = self.row_ptr;
        self.set_row(p, v);
        self.row_ptr += 1;
    }

    // §53: specials pass, no_op is dropped
    fn special(&mut self, gf_com: i32) -> Result<(), String> {
        match gf_com {
            XXX1..=242 => {
                self.pk_byte(gf_com - XXX1 + PK_XXX1);
                let mut i: i32 = 0;
                for _ in 0..=gf_com - XXX1 {
                    let k = self.gf_byte()?;
                    self.pk_byte(k);
                    i = i.wrapping_mul(256).wrapping_add(k);
                }
                for _ in 0..i.max(0) {
                    let b = self.gf_byte()?;
                    self.pk_byte(b);
                }
            }
            YYY => {
                self.pk_byte(PK_YYY);
                let q = self.gf_signed_quad()?;
                self.pk_word(q);
            }
            _ => {} // no_op
        }
        Ok(())
    }

    // §51
    fn convert_gf_file(&mut self) -> Result<(), String> {
        self.gf_pos = 0;
        if self.gf_byte()? != PRE {
            return Err(bad_gf("First byte is not preamble"));
        }
        if self.gf_byte()? != GF_ID_BYTE {
            return Err(bad_gf("Identification byte is incorrect"));
        }
        self.find_and_interpret_postamble()?;
        self.move_to_byte(2);
        // open_pk_file (§40)
        self.pk_open = true;
        self.write_preamble()?;
        loop {
            let gf_com = self.gf_byte()?;
            match gf_com {
                BOC | BOC1 => self.interpret_character(gf_com)?,
                XXX1..=NO_OP => self.special(gf_com)?,
                POST => {}
                _ => {
                    return Err(bad_gf(&format!(
                        "Unexpected {gf_com} command between characters"
                    )));
                }
            }
            if gf_com == POST {
                break;
            }
        }
        // §84
        self.pk_byte(PK_POST);
        while self.pk_loc() % 4 != 0 {
            self.pk_byte(PK_NO_OP);
        }
        Ok(())
    }

    // §54, §57
    fn interpret_character(&mut self, gf_com: i32) -> Result<(), String> {
        if gf_com == BOC {
            self.gf_ch = self.gf_signed_quad()?;
            self.gf_signed_quad()?; // dispose of back pointer
            self.min_m = self.gf_signed_quad()?;
            self.max_m = self.gf_signed_quad()?;
            self.min_n = self.gf_signed_quad()?;
            self.max_n = self.gf_signed_quad()?;
        } else {
            self.gf_ch = self.gf_byte()?;
            let i = self.gf_byte()?;
            self.max_m = self.gf_byte()?;
            self.min_m = self.max_m - i;
            let i = self.gf_byte()?;
            self.max_n = self.gf_byte()?;
            self.min_n = self.max_n - i;
        }
        self.gf_ch_mod_256 = if self.gf_ch >= 0 {
            self.gf_ch % 256
        } else {
            255 - ((-(1 + self.gf_ch)) % 256)
        };
        if self.status[self.gf_ch_mod_256 as usize] == VIRGIN {
            return Err(bad_gf(&format!(
                "no character locator for character {}",
                self.gf_ch
            )));
        }
        // §57: Convert character to packed form
        self.row_ptr = 2;
        let mut on = false;
        let mut extra: i32 = 0;
        let mut state = true;
        loop {
            let gf_com = self.gf_byte()?;
            match gf_com {
                // §59
                PAINT_0 => {
                    state = !state;
                    on = !on;
                }
                1..=66 => {
                    let i = if gf_com < PAINT1 {
                        gf_com - PAINT_0
                    } else {
                        let mut i: i32 = 0;
                        for _ in 0..=gf_com - PAINT1 {
                            i = i * 256 + self.gf_byte()?;
                        }
                        i
                    };
                    if state {
                        extra += i;
                        state = false;
                    } else {
                        self.put_in_rows(extra);
                        extra = i;
                    }
                    on = !on;
                }
                SKIP0..=73 => {
                    let mut i: i32 = 0;
                    for _ in 1..=gf_com - SKIP0 {
                        i = i * 256 + self.gf_byte()?;
                    }
                    if on == state {
                        self.put_in_rows(extra);
                    }
                    for _ in 0..=i {
                        self.put_in_rows(END_OF_ROW);
                    }
                    on = false;
                    extra = 0;
                    state = true;
                }
                NEW_ROW_0..=MAX_NEW_ROW => {
                    if on == state {
                        self.put_in_rows(extra);
                    }
                    self.put_in_rows(END_OF_ROW);
                    on = true;
                    extra = gf_com - NEW_ROW_0;
                    state = false;
                }
                XXX1..=NO_OP => self.special(gf_com)?,
                EOC => {
                    if on == state {
                        self.put_in_rows(extra);
                    }
                    if self.row_ptr > 2 && self.row(self.row_ptr - 1) != END_OF_ROW {
                        self.put_in_rows(END_OF_ROW);
                    }
                    self.put_in_rows(END_OF_CHAR);
                    self.pack_and_send_character();
                    self.status[self.gf_ch_mod_256 as usize] = SENT;
                    if self.pk_loc() != self.pred_pk_loc {
                        return Err("Internal error while writing character!".into());
                    }
                }
                _ => {
                    return Err(bad_gf(&format!(
                        "Unexpected {gf_com} command in character definition"
                    )));
                }
            }
            if gf_com == EOC {
                return Ok(());
            }
        }
    }

    // §60
    fn find_and_interpret_postamble(&mut self) -> Result<(), String> {
        self.gf_len = i32::try_from(self.gf.len()).map_err(|_| bad_gf("too long"))?;
        if self.gf_len < 8 {
            return Err(bad_gf(&format!("only {} bytes long", self.gf_len)));
        }
        let mut post_loc = self.gf_len - 4;
        let mut k;
        loop {
            if post_loc == 0 {
                return Err(bad_gf("all 223's"));
            }
            self.move_to_byte(post_loc);
            k = self.gf_byte()?;
            post_loc -= 1;
            if k != 223 {
                break;
            }
        }
        if k != GF_ID_BYTE {
            return Err(bad_gf(&format!("ID byte is {k}")));
        }
        if post_loc < 5 {
            return Err(bad_gf(&format!("post location is {post_loc}")));
        }
        self.move_to_byte(post_loc - 3);
        let q = self.gf_signed_quad()?;
        if q < 0 || q > post_loc - 3 {
            return Err(bad_gf(&format!("post pointer is {q}")));
        }
        self.move_to_byte(q);
        k = self.gf_byte()?;
        if k != POST {
            return Err(bad_gf(&format!("byte at {q} is not post")));
        }
        self.gf_signed_quad()?; // skip over junk
        self.design_size = self.gf_signed_quad()?;
        self.check_sum = self.gf_signed_quad()?;
        self.hppp = self.gf_signed_quad()?;
        self.vppp = self.gf_signed_quad()?;
        for _ in 0..4 {
            self.gf_signed_quad()?; // skip over junk
        }
        loop {
            let gf_com = self.gf_byte()?;
            match gf_com {
                CHAR_LOC | CHAR_LOC0 => {
                    let ch = self.gf_byte()? as usize;
                    if self.status[ch] != VIRGIN {
                        return Err(bad_gf("Locator for this character already found."));
                    }
                    if gf_com == CHAR_LOC {
                        self.dx[ch] = self.gf_signed_quad()?;
                        self.dy[ch] = self.gf_signed_quad()?;
                    } else {
                        self.dx[ch] = self.gf_byte()? * 65536;
                        self.dy[ch] = 0;
                    }
                    self.tfm_width[ch] = self.gf_signed_quad()?;
                    self.gf_signed_quad()?;
                    self.status[ch] = LOCATED;
                    self.gf_ch = ch as i32;
                }
                XXX1..=NO_OP => self.special(gf_com)?,
                POST_POST => return Ok(()),
                _ => {
                    return Err(bad_gf(&format!("Unexpected {gf_com} in postamble")));
                }
            }
        }
    }

    // §82: the PK preamble, its comment the GF one's (gftopk.ch §81:
    // `comm_length` and `from_length` are 0)
    fn write_preamble(&mut self) -> Result<(), String> {
        const COMM_LENGTH: i32 = 0;
        const FROM_LENGTH: i32 = 0;
        self.pk_byte(PK_PRE);
        self.pk_byte(PK_ID);
        let mut i = self.gf_byte()?; // get length of introductory comment
        let mut j;
        loop {
            j = if i == 0 {
                i32::from(b'.')
            } else {
                self.gf_byte()?
            };
            i -= 1;
            if j != i32::from(b' ') {
                break;
            }
        } // remove leading blanks
        i += 1; // this many bytes to copy
        let k = if i == 0 {
            COMM_LENGTH - FROM_LENGTH
        } else {
            i + COMM_LENGTH
        };
        self.pk_byte(if k > 255 { 255 } else { k });
        for k in 1..=i {
            if k > 1 {
                j = self.gf_byte()?;
            }
            if k < 256 - COMM_LENGTH {
                self.pk_byte(j);
            }
        }
        self.pk_word(self.design_size);
        self.pk_word(self.check_sum);
        self.pk_word(self.hppp);
        self.pk_word(self.vppp);
        Ok(())
    }

    // §62 (the WEB's put_count leaves its pointers set after the last count)
    #[allow(unused_assignments)]
    fn pack_and_send_character(&mut self) {
        let mut extra: i32 = 0;
        let mut put_ptr: i32;
        let mut repeat_flag: i32;
        let mut h_bit: i32;
        let mut buff: i32;
        let mut dyn_f: i32;
        let height: i32;
        let width: i32;
        let x_offset: i32;
        let y_offset: i32;
        let mut deriv = [0i32; 14];
        let mut b_comp_size: i32;
        let mut flag_byte: i32;
        let mut state: bool;
        let mut on: bool;
        let mut comp_size: i32;
        let mut count: i32;
        let mut p_bit: i32;
        let (mut r_on, mut s_on) = (false, false);
        let (mut r_count, mut s_count) = (0i32, 0i32);
        let (mut r_i, mut s_i) = (0i32, 0i32);
        let max_2: i32;
        let (mut i, mut j, mut k): (i32, i32, i32);

        // §63: Scan for bounding box
        i = 2;
        self.row_ptr -= 1;
        while self.row(i) == END_OF_ROW {
            i += 1;
        }
        if self.row(i) != END_OF_CHAR {
            self.max_n = self.max_n - i + 2;
            while self.row(self.row_ptr - 2) == END_OF_ROW {
                self.row_ptr -= 1;
                let p = self.row_ptr;
                self.set_row(p, END_OF_CHAR);
            }
            self.min_n = self.max_n + 1;
            extra = self.max_m - self.min_m + 1;
            self.max_m = 0;
            j = i;
            while self.row(j) != END_OF_CHAR {
                self.min_n -= 1;
                if self.row(j) != END_OF_ROW {
                    k = self.row(j);
                    if k < extra {
                        extra = k;
                    }
                    j += 1;
                    while self.row(j) != END_OF_ROW {
                        k += self.row(j);
                        j += 1;
                    }
                    if self.max_m < k {
                        self.max_m = k;
                    }
                }
                j += 1;
            }
            self.min_m += extra;
            self.max_m = self.min_m + self.max_m - 1 - extra;
            height = self.max_n - self.min_n + 1;
            width = self.max_m - self.min_m + 1;
            x_offset = -self.min_m;
            y_offset = self.max_n;
        } else {
            height = 0;
            width = 0;
            x_offset = 0;
            y_offset = 0;
        }

        // §64: Convert row-list to glyph-list
        macro_rules! put_count {
            ($v:expr) => {{
                let v = $v;
                self.set_row(put_ptr, v);
                put_ptr += 1;
                if repeat_flag > 0 {
                    self.set_row(put_ptr, -repeat_flag);
                    repeat_flag = 0;
                    put_ptr += 1;
                }
            }};
        }
        put_ptr = 0;
        self.row_ptr = 2;
        repeat_flag = 0;
        state = true;
        buff = 0;
        while self.row(self.row_ptr) == END_OF_ROW {
            self.row_ptr += 1;
        }
        while self.row(self.row_ptr) != END_OF_CHAR {
            // §66: Skip over repeated rows
            i = self.row_ptr;
            if self.row(i) != END_OF_ROW && (self.row(i) != extra || self.row(i + 1) != width) {
                j = i + 1;
                while self.row(j - 1) != END_OF_ROW {
                    j += 1;
                }
                while self.row(i) == self.row(j) {
                    if self.row(i) == END_OF_ROW {
                        repeat_flag += 1;
                        self.row_ptr = i + 1;
                    }
                    i += 1;
                    j += 1;
                }
            }
            // §67: Reformat count list
            if self.row(self.row_ptr) != END_OF_ROW {
                let p = self.row_ptr;
                self.set_row(p, self.row(p) - extra);
            }
            h_bit = 0;
            while self.row(self.row_ptr) != END_OF_ROW {
                let r = self.row(self.row_ptr);
                h_bit += r;
                if state {
                    buff += r;
                    state = false;
                } else if r > 0 {
                    put_count!(buff);
                    buff = r;
                } else {
                    state = true;
                }
                self.row_ptr += 1;
            }
            if h_bit < width {
                if state {
                    buff += width - h_bit;
                } else {
                    put_count!(buff);
                    buff = width - h_bit;
                    state = true;
                }
            } else {
                state = false;
            }
            self.row_ptr += 1;
        }
        if buff > 0 {
            put_count!(buff);
        }
        put_count!(END_OF_CHAR);

        // §68: Calculate dyn_f and packed size and write character
        for d in deriv.iter_mut().skip(1) {
            *d = 0;
        }
        i = 0;
        let first_on = self.row(i) == 0;
        if first_on {
            i += 1;
        }
        comp_size = 0;
        while self.row(i) != END_OF_CHAR {
            // §69: Process count for best dyn_f value
            j = self.row(i);
            if j == -1 {
                comp_size += 1;
            } else {
                if j < 0 {
                    comp_size += 1;
                    j = -j;
                }
                if j < 209 {
                    comp_size += 2;
                } else {
                    k = j - 193;
                    while k >= 16 {
                        k /= 16;
                        comp_size += 2;
                    }
                    comp_size += 1;
                }
                if j < 14 {
                    deriv[j as usize] -= 1;
                } else if j < 209 {
                    deriv[((223 - j) / 15) as usize] += 1;
                } else {
                    k = 16;
                    while k * 16 < j + 3 {
                        k *= 16;
                    }
                    if j - k <= 192 {
                        deriv[((207 - j + k) / 15) as usize] += 2;
                    }
                }
            }
            i += 1;
        }
        b_comp_size = comp_size;
        dyn_f = 0;
        for (n, d) in deriv.iter().enumerate().skip(1) {
            comp_size += d;
            if comp_size <= b_comp_size {
                b_comp_size = comp_size;
                dyn_f = n as i32;
            }
        }
        comp_size = (b_comp_size + 1) / 2;
        if comp_size > (height * width + 7) / 8 || height * width == 0 {
            comp_size = (height * width + 7) / 8;
            dyn_f = 14;
        }

        // §71: Write character preamble
        let c = self.gf_ch_mod_256 as usize;
        flag_byte = dyn_f * 16;
        if first_on {
            flag_byte += 8;
        }
        if self.gf_ch != self.gf_ch_mod_256
            || self.tfm_width[c] > 16777215
            || self.tfm_width[c] < 0
            || self.dy[c] != 0
            || self.dx[c] < 0
            || self.dx[c] % 65536 != 0
            || comp_size > 196594
            || width > 65535
            || height > 65535
            || x_offset > 32767
            || y_offset > 32767
            || x_offset < -32768
            || y_offset < -32768
        {
            // §72: long character preamble
            flag_byte += 7;
            self.pk_byte(flag_byte);
            comp_size += 28;
            self.pk_word(comp_size);
            self.pk_word(self.gf_ch);
            self.pred_pk_loc = self.pk_loc() + comp_size;
            self.pk_word(self.tfm_width[c]);
            self.pk_word(self.dx[c]);
            self.pk_word(self.dy[c]);
            self.pk_word(width);
            self.pk_word(height);
            self.pk_word(x_offset);
            self.pk_word(y_offset);
        } else if self.dx[c] > 16777215
            || width > 255
            || height > 255
            || x_offset > 127
            || y_offset > 127
            || x_offset < -128
            || y_offset < -128
            || comp_size > 1015
        {
            // §74: two-byte short character preamble
            comp_size += 13;
            flag_byte += comp_size / 65536 + 4;
            self.pk_byte(flag_byte);
            self.pk_halfword(comp_size % 65536);
            self.pk_byte(self.gf_ch);
            self.pred_pk_loc = self.pk_loc() + comp_size;
            self.pk_three_bytes(self.tfm_width[c]);
            self.pk_halfword(self.dx[c] / 65536);
            self.pk_halfword(width);
            self.pk_halfword(height);
            self.pk_halfword(x_offset);
            self.pk_halfword(y_offset);
        } else {
            // §73: one-byte short character preamble
            comp_size += 8;
            flag_byte += comp_size / 256;
            self.pk_byte(flag_byte);
            self.pk_byte(comp_size % 256);
            self.pk_byte(self.gf_ch);
            self.pred_pk_loc = self.pk_loc() + comp_size;
            self.pk_three_bytes(self.tfm_width[c]);
            self.pk_byte(self.dx[c] / 65536);
            self.pk_byte(width);
            self.pk_byte(height);
            self.pk_byte(x_offset);
            self.pk_byte(y_offset);
        }

        if dyn_f != 14 {
            // §75: Send compressed format
            self.bit_weight = 16;
            max_2 = 208 - 15 * dyn_f;
            i = 0;
            if self.row(i) == 0 {
                i += 1;
            }
            while self.row(i) != END_OF_CHAR {
                j = self.row(i);
                if j == -1 {
                    self.pk_nyb(15);
                } else {
                    if j < 0 {
                        self.pk_nyb(14);
                        j = -j;
                    }
                    if j <= dyn_f {
                        self.pk_nyb(j);
                    } else if j <= max_2 {
                        j = j - dyn_f - 1;
                        self.pk_nyb(j / 16 + dyn_f + 1);
                        self.pk_nyb(j % 16);
                    } else {
                        j = j - max_2 + 15;
                        k = 16;
                        while k <= j {
                            k *= 16;
                            self.pk_nyb(0);
                        }
                        while k > 1 {
                            k /= 16;
                            self.pk_nyb(j / k);
                            j %= k;
                        }
                    }
                }
                i += 1;
            }
            if self.bit_weight != 16 {
                self.pk_byte(self.output_byte);
            }
        } else if height > 0 {
            // §76: Send bit map
            buff = 0;
            p_bit = 8;
            i = 1;
            h_bit = width;
            on = false;
            state = false;
            count = self.row(0);
            repeat_flag = 0;
            while self.row(i) != END_OF_CHAR || state || count > 0 {
                if state {
                    count = r_count;
                    i = r_i;
                    on = r_on;
                    repeat_flag -= 1;
                } else {
                    r_count = count;
                    r_i = i;
                    r_on = on;
                }
                // §79: Send one row by bits
                loop {
                    if count == 0 {
                        if self.row(i) < 0 {
                            if !state {
                                repeat_flag = -self.row(i);
                            }
                            i += 1;
                        }
                        count = self.row(i);
                        i += 1;
                        on = !on;
                    }
                    if count >= p_bit && p_bit < h_bit {
                        // we end a byte, we don't end the row
                        if on {
                            buff += self.power[p_bit as usize] - 1;
                        }
                        self.pk_byte(buff);
                        buff = 0;
                        h_bit -= p_bit;
                        count -= p_bit;
                        p_bit = 8;
                    } else if count < p_bit && count < h_bit {
                        // we end neither the row nor the byte
                        if on {
                            buff +=
                                self.power[p_bit as usize] - self.power[(p_bit - count) as usize];
                        }
                        p_bit -= count;
                        h_bit -= count;
                        count = 0;
                    } else {
                        // we end a row and maybe a byte
                        if on {
                            buff +=
                                self.power[p_bit as usize] - self.power[(p_bit - h_bit) as usize];
                        }
                        count -= h_bit;
                        p_bit -= h_bit;
                        h_bit = width;
                        if p_bit == 0 {
                            self.pk_byte(buff);
                            buff = 0;
                            p_bit = 8;
                        }
                    }
                    if h_bit == width {
                        break;
                    }
                }
                if state && repeat_flag == 0 {
                    count = s_count;
                    i = s_i;
                    on = s_on;
                    state = false;
                } else if !state && repeat_flag > 0 {
                    s_count = count;
                    s_i = i;
                    s_on = on;
                    state = true;
                }
            }
            if p_bit != 8 {
                self.pk_byte(buff);
            }
        }
    }
}
