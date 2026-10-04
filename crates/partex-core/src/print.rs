//! Part 5: On-line and off-line printing (§54–§71), as changed by web2c.

use crate::diag::DIAG;
use crate::host::Host;
#[allow(unused_imports)]
use crate::params::Flavor;
use crate::tex::{OUT_CHUNK, Tex};
use crate::track::{Output, Tracker};
pub use crate::web::{LOG_ONLY, NEW_STRING, NO_PRINT, PSEUDO, TERM_AND_LOG, TERM_ONLY};

/// `print("???")`: what `print` shows for an impossible string number.
const QQQ: &[u8] = b"???";

impl<H: Host, T: Tracker> Tex<H, T> {
    /// A read of scalar row `row`, whose value is `v`, for a tracker that
    /// keeps versions (DESIGN 7.17.12: a scalar slot, its version its
    /// value).
    #[inline]
    pub(crate) fn scalar_read(&self, row: crate::track::Row, v: i32) {
        if T::VALUES {
            self.tracker
                .row_read(row, || crate::track::scalar_version_i32(v));
        }
    }

    /// Scalar row `row` was written and holds `v`.
    #[inline]
    pub(crate) fn scalar_wrote(&self, row: crate::track::Row, v: i32) {
        if T::VALUES {
            self.tracker
                .row_wrote(row, crate::track::scalar_version_i32(v));
        }
    }

    /// Reads of the printing columns about to change (`term`, `file`).
    #[inline]
    fn offsets_read(&self, term: bool, file: bool) {
        // (with the columns the link's, they are not state: `effects/flow.rs`)
        if self.flow.on {
            return;
        }
        if term {
            self.scalar_read(
                crate::track::Row::Scalar(crate::track::scalar::TERM_OFFSET),
                self.term_offset,
            );
        }
        if file {
            self.scalar_read(
                crate::track::Row::Scalar(crate::track::scalar::FILE_OFFSET),
                self.file_offset,
            );
        }
    }

    /// The printing columns changed (`term`, `file`).
    #[inline]
    pub(crate) fn offsets_wrote(&self, term: bool, file: bool) {
        if self.flow.on {
            return;
        }
        if term {
            self.scalar_wrote(
                crate::track::Row::Scalar(crate::track::scalar::TERM_OFFSET),
                self.term_offset,
            );
        }
        if file {
            self.scalar_wrote(
                crate::track::Row::Scalar(crate::track::scalar::FILE_OFFSET),
                self.file_offset,
            );
        }
    }

    /// `output_active` (§989), read.
    pub(crate) fn output_active(&self) -> bool {
        self.scalar_read(
            crate::track::Row::Scalar(crate::track::scalar::OUTPUT_ACTIVE),
            i32::from(self.output_active),
        );
        self.output_active
    }

    /// `output_active` set.
    pub(crate) fn set_output_active(&mut self, v: bool) {
        self.output_active = v;
        self.scalar_wrote(
            crate::track::Row::Scalar(crate::track::scalar::OUTPUT_ACTIVE),
            i32::from(v),
        );
    }

    /// The printing columns, read (where a message decides whether to
    /// start a new line).
    pub(crate) fn offsets(&self) -> (i32, i32) {
        self.offsets_read(true, true);
        (self.term_offset, self.file_offset)
    }

    /// §55: initialize the output routines.
    pub(crate) fn init_output(&mut self) {
        self.set_selector(TERM_ONLY);
        self.tally = 0;
        self.term_offset = 0;
        self.file_offset = 0;
        self.offsets_wrote(true, true);
        self.flow_op(&[crate::effects::flow::COL0, 3]);
    }

    /// The outputs printing to the selector goes to, as a flow's mask (1
    /// the terminal, 2 the log), if printing makes flow ops.
    fn flow_mask(&self) -> u8 {
        if !self.flow.on {
            return 0;
        }
        match self.selector() {
            TERM_ONLY => 1,
            LOG_ONLY => 2,
            TERM_AND_LOG => 3,
            _ => 0,
        }
    }

    /// Op bytes `op` for the flow (`effects/flow.rs`), unless the decision
    /// being made has made its op.
    pub(crate) fn flow_op(&mut self, op: &[u8]) {
        if !self.flow.on || self.flow.quiet {
            return;
        }
        let log = self.log_file.id;
        if let Some(fx) = &mut self.effects
            && self.flow.push(fx, log, op)
        {
            self.flow_began();
        }
    }

    /// A flow began: the log's being open, which it names, read (in a
    /// step run again, a read the slot is placed for: the arrays may hold
    /// the log closed, as the job's end left it).
    fn flow_began(&self) {
        self.out_read(crate::streams::LOG);
    }

    /// A column decision's op (`SEP`, `NLC`), then `f` deciding for the
    /// engine's own columns, its printing making no ops.
    fn flow_decide(&mut self, op: &[u8], f: impl FnOnce(&mut Self)) {
        self.flow_op(op);
        let quiet = core::mem::replace(&mut self.flow.quiet, true);
        f(self);
        self.flow.quiet = quiet;
    }

    /// The space or new line before a message (§638, §1280 and others):
    /// a new line if the terminal's column is past `thr`, else a space if
    /// either column is past its start.
    pub(crate) fn print_sep(&mut self, thr: i32) {
        let mask = self.flow_mask();
        let plain = |t: &mut Self| {
            let (term_offset, file_offset) = t.offsets();
            if term_offset > thr {
                t.print_ln();
            } else if term_offset > 0 || file_offset > 0 {
                t.print_char(b' ');
            }
        };
        if mask == 0 {
            plain(self);
            return;
        }
        let mut op = alloc::vec![crate::effects::flow::SEP, mask];
        op.extend_from_slice(&thr.to_le_bytes());
        op.push(u8::from(self.new_line_char() == i32::from(b' ')));
        self.flow_decide(&op, plain);
    }

    // §56: `wterm`, `wlog` and friends.

    /// `wterm`: raw bytes to the terminal.
    pub(crate) fn term_bytes(&mut self, s: &[u8]) {
        if self.flow.on {
            let mut op = alloc::vec![crate::effects::flow::RAW, 1];
            op.extend_from_slice(&u32::try_from(s.len()).unwrap_or(0).to_le_bytes());
            op.extend_from_slice(s);
            self.flow_op(&op);
            return;
        }
        if T::VALUES {
            // (the terminal's bytes are effects, never read)
            self.tracker.output(Output::Term, s);
        }
        self.term_buf.extend_from_slice(s);
    }

    /// Bytes to the terminal as an effect emitted again (a hit's): not an
    /// effect of the running call a second time.
    pub(crate) fn term_bytes_raw(&mut self, s: &[u8]) {
        self.term_buf.extend_from_slice(s);
    }

    /// Bytes to the log as an effect emitted again (a hit's).
    pub(crate) fn log_bytes_raw(&mut self, s: &[u8]) {
        for &c in s {
            self.log_file.buf.push(c);
            if self.log_file.buf.len() >= OUT_CHUNK {
                self.flush_log();
            }
        }
    }

    fn wterm(&mut self, c: u8) {
        if T::VALUES {
            self.tracker.output(Output::Term, &[c]);
        }
        self.term_buf.push(c);
    }

    fn wterm_cr(&mut self) {
        self.wterm(b'\n');
    }

    fn wlog(&mut self, c: u8) {
        if T::VALUES {
            // (the log's bytes are effects, never read)
            self.tracker.output(Output::Log, &[c]);
        }
        self.log_file.buf.push(c);
        if self.log_file.buf.len() >= OUT_CHUNK {
            self.flush_log();
        }
    }

    /// `wlog` of a Pascal string: straight to the log, not counted in
    /// `file_offset`.
    pub(crate) fn wlog_bytes(&mut self, s: &[u8]) {
        if self.flow.on {
            let mut op = alloc::vec![crate::effects::flow::RAW, 2];
            op.extend_from_slice(&u32::try_from(s.len()).unwrap_or(0).to_le_bytes());
            op.extend_from_slice(s);
            self.flow_op(&op);
            return;
        }
        for &c in s {
            self.wlog(c);
        }
    }

    fn wlog_cr(&mut self) {
        self.wlog(b'\n');
    }

    /// Bytes to `\\write` stream `stream`'s file as an effect emitted
    /// again (a hit's).
    pub(crate) fn write_bytes_raw(&mut self, stream: usize, s: &[u8]) {
        self.write_file[stream & 15].buf.extend_from_slice(s);
        self.write_file_spill(stream & 15);
    }

    fn write_to(&mut self, stream: usize, c: u8) {
        // (an effect, and at a line's end a store: `streams`)
        self.stream_byte(stream, c);
        self.write_file[stream].buf.push(c);
        self.write_file_spill(stream);
    }

    /// `\write` file `stream`'s buffer to the host once it is full.
    fn write_file_spill(&mut self, stream: usize) {
        let f = &mut self.write_file[stream];
        if f.buf.len() >= OUT_CHUNK
            && let Some(id) = f.id
        {
            let mut buf = core::mem::take(&mut f.buf);
            self.write_file_bytes(id, &buf);
            buf.clear();
            self.write_file[stream].buf = buf;
        }
    }

    pub(crate) fn flush_log(&mut self) {
        let mut buf = core::mem::take(&mut self.log_file.buf);
        if let Some(id) = self.log_file.id {
            self.out_write(id, &buf);
        }
        buf.clear();
        self.log_file.buf = buf;
    }

    /// Hand every buffered output (log, `\write` files, terminal) to the
    /// host.
    pub(crate) fn flush_outputs(&mut self) {
        // (the log's bytes are effects, never read; the file they go to is
        // the log's being open, `Out(LOG)`, read)
        if !self.log_file.buf.is_empty() {
            self.out_read(crate::streams::LOG);
        }
        if self.log_file.id.is_some() {
            self.flush_log();
        }
        // (`\write` files go to the host even with effects on: the job
        // may read them back, so they are state, `effects.rs`)
        for k in 0..self.write_file.len() {
            let f = &mut self.write_file[k];
            if let Some(id) = f.id
                && !f.buf.is_empty()
            {
                let mut buf = core::mem::take(&mut f.buf);
                self.write_file_bytes(id, &buf);
                buf.clear();
                self.write_file[k].buf = buf;
            }
        }
        self.update_terminal();
    }

    /// §34: `update_terminal` (web2c: `fflush(stdout)`).
    pub(crate) fn update_terminal(&mut self) {
        if !self.term_buf.is_empty() {
            let mut buf = core::mem::take(&mut self.term_buf);
            self.out_term(&buf);
            buf.clear();
            self.term_buf = buf;
        }
    }

    /// §57: end a line of output.
    pub(crate) fn print_ln(&mut self) {
        if self.selector() < PSEUDO {
            self.memo.printed += 1;
        }
        let selector = self.selector();
        if let Some(d) = &mut self.diag
            && (d.capturing || selector == DIAG)
        {
            d.buf.push(b'\n');
        }
        let mask = self.flow_mask();
        if mask != 0 {
            self.flow_op(&[crate::effects::flow::NL, mask]);
            if mask & 1 != 0 {
                self.term_offset = 0;
            }
            if mask & 2 != 0 {
                self.file_offset = 0;
            }
            return;
        }
        match self.selector() {
            TERM_AND_LOG => {
                self.wterm_cr();
                self.wlog_cr();
                self.term_offset = 0;
                self.file_offset = 0;
                self.offsets_wrote(true, true);
            }
            LOG_ONLY => {
                self.wlog_cr();
                self.file_offset = 0;
                self.offsets_wrote(false, true);
            }
            TERM_ONLY => {
                self.wterm_cr();
                self.term_offset = 0;
                self.offsets_wrote(true, false);
            }
            NO_PRINT | PSEUDO | NEW_STRING | DIAG => {}
            s => self.write_to(stream(s), b'\n'),
        }
    }

    /// §58: print one character. All printing comes through here or
    /// `print_ln`.
    pub(crate) fn print_char(&mut self, s: u8) {
        if self.selector() < PSEUDO {
            self.memo.printed += 1;
        }
        if i32::from(s) == self.new_line_char() && self.selector() < PSEUDO {
            self.print_ln();
            return;
        }
        let selector = self.selector();
        if let Some(d) = &mut self.diag
            && (d.capturing || selector == DIAG)
        {
            // A copy for the structured diagnostic (see `diag.rs`).
            d.buf.push(s);
            if self.selector() == DIAG {
                self.tally += 1;
                return;
            }
        }
        let x = self.xchr[usize::from(s)];
        let max_print_line = self.params.max_print_line;
        let mask = self.flow_mask();
        if mask != 0 {
            // (the character to the flow, the wraps the link's; the
            // engine's own columns kept as TeX keeps them)
            if !self.flow.quiet {
                let log = self.log_file.id;
                if let Some(fx) = &mut self.effects
                    && self.flow.text(fx, log, mask, x)
                {
                    self.flow_began();
                }
            }
            if mask & 1 != 0 {
                self.term_offset += 1;
            }
            if mask & 2 != 0 {
                self.file_offset += 1;
            }
            if mask == 3 {
                if self.term_offset == max_print_line {
                    self.term_offset = 0;
                }
                if self.file_offset == max_print_line {
                    self.file_offset = 0;
                }
            } else if self.term_offset == max_print_line || self.file_offset == max_print_line {
                let quiet = core::mem::replace(&mut self.flow.quiet, true);
                self.print_ln();
                self.flow.quiet = quiet;
            }
            self.tally += 1;
            return;
        }
        match self.selector() {
            TERM_AND_LOG => {
                self.offsets_read(true, true);
                self.wterm(x);
                self.wlog(x);
                self.term_offset += 1;
                self.file_offset += 1;
                if self.term_offset == max_print_line {
                    self.wterm_cr();
                    self.term_offset = 0;
                }
                if self.file_offset == max_print_line {
                    self.wlog_cr();
                    self.file_offset = 0;
                }
                self.offsets_wrote(true, true);
            }
            LOG_ONLY => {
                self.offsets_read(false, true);
                self.wlog(x);
                self.file_offset += 1;
                self.offsets_wrote(false, true);
                if self.file_offset == max_print_line {
                    self.print_ln();
                }
            }
            TERM_ONLY => {
                self.offsets_read(true, false);
                self.wterm(x);
                self.term_offset += 1;
                self.offsets_wrote(true, false);
                if self.term_offset == max_print_line {
                    self.print_ln();
                }
            }
            NO_PRINT => {}
            PSEUDO => {
                if self.tally < self.trick_count {
                    let i = usize::try_from(self.tally % self.params.error_line).unwrap_or(0);
                    self.trick_buf[i] = s;
                }
            }
            NEW_STRING => {
                // We drop characters if the string space is full.
                if self.pool_ptr < self.pool_size() {
                    self.append_char(s);
                }
            }
            sel => self.write_to(stream(sel), x),
        }
        self.tally += 1;
    }

    /// §59: print string number `s`.
    ///
    /// For `s < 256` this follows web2c's version: printable characters
    /// (`xprn`) go out directly, others in `^^` notation. encTeX's
    /// `\mubytelog` path is omitted (encTeX is not supported yet).
    pub(crate) fn print(&mut self, s: i32) {
        let Ok(u) = usize::try_from(s) else {
            self.print_str(QQQ); // can't happen
            return;
        };
        if u >= self.str_ptr {
            self.print_str(QQQ); // this can't happen
            return;
        }
        if u >= 256 {
            self.print_pool(u);
            return;
        }
        let c = u8::try_from(u).unwrap_or(0);
        if self.selector() > PSEUDO && !self.special_printing && !self.message_printing {
            self.print_char(c); // internal strings are not expanded
            return;
        }
        if s == self.new_line_char() {
            if self.selector() < PSEUDO {
                self.print_ln();
                self.no_convert = false;
                return;
            } else if self.message_printing {
                self.print_char(c);
                self.no_convert = false;
                return;
            }
        }
        if self.xprn[u] || self.special_printing {
            self.print_char(c);
            self.no_convert = false;
            return;
        }
        self.no_convert = false;
        let nl = self.new_line_char();
        // Temporarily disable the new-line character.
        self.set_int_par(crate::eqtb::NEW_LINE_CHAR_CODE, -1);
        self.print_pool(u);
        self.set_int_par(crate::eqtb::NEW_LINE_CHAR_CODE, nl);
    }

    /// The loop at the end of §59: `print_char` every character of `s`.
    fn print_pool(&mut self, s: usize) {
        for j in self.str_start[s]..self.str_start[s + 1] {
            self.print_char(self.str_pool[j]);
        }
    }

    /// §59 `print` of a WEB string literal given by its contents. As in
    /// WEB, a one-character literal is a character code, not a pool string.
    pub(crate) fn print_str(&mut self, s: &[u8]) {
        if let [c] = s {
            self.print(i32::from(*c));
            return;
        }
        for &c in s {
            self.print_char(c);
        }
    }

    /// §60: print a string that may contain unprintable characters.
    pub(crate) fn slow_print(&mut self, s: i32) {
        match usize::try_from(s) {
            Ok(u) if u >= 256 && u < self.str_ptr => {
                for j in self.str_start[u]..self.str_start[u + 1] {
                    self.print(i32::from(self.str_pool[j]));
                }
            }
            _ => self.print(s),
        }
    }

    /// §62: print `s` at the beginning of a line.
    pub(crate) fn print_nl(&mut self, s: &[u8]) {
        self.new_line_if_needed();
        self.print_str(s);
    }

    /// §62 for a string number.
    pub(crate) fn print_nl_num(&mut self, s: i32) {
        self.new_line_if_needed();
        self.print(s);
    }

    fn new_line_if_needed(&mut self) {
        let mask = self.flow_mask();
        if mask != 0 {
            self.flow_decide(&[crate::effects::flow::NLC, mask], Self::new_line_plain);
            return;
        }
        self.new_line_plain();
    }

    fn new_line_plain(&mut self) {
        // pdfTeX §62 also ends a line written to a `\write` file.
        let write_file = self.params.flavor == Flavor::PdfTex && self.selector() < NO_PRINT;
        if !write_file {
            let s = self.selector();
            self.offsets_read(s % 2 == 1, s >= LOG_ONLY);
        }
        if write_file
            || (self.term_offset > 0 && self.selector() % 2 == 1)
            || (self.file_offset > 0 && self.selector() >= LOG_ONLY)
        {
            self.print_ln();
        }
    }

    /// §63: print the escape character, then `s`.
    pub(crate) fn print_esc(&mut self, s: &[u8]) {
        self.print_escape_char();
        for &c in s {
            // `slow_print` of a pool string prints each character via `print`.
            self.print(i32::from(c));
        }
    }

    /// §63 for a string number.
    pub(crate) fn print_esc_num(&mut self, s: i32) {
        self.print_escape_char();
        self.slow_print(s);
    }

    fn print_escape_char(&mut self) {
        // §243: `c:=escape_char`.
        let c = self.escape_char();
        if (0..256).contains(&c) {
            self.print(c);
        }
    }

    /// §64: print `dig[k-1]`…`dig[0]`.
    fn print_the_digs(&mut self, mut k: usize) {
        while k > 0 {
            k -= 1;
            let d = self.dig[k];
            if d < 10 {
                self.print_char(b'0' + d);
            } else {
                self.print_char(b'A' - 10 + d);
            }
        }
    }

    /// §65: print an integer in decimal.
    pub(crate) fn print_int(&mut self, n: i32) {
        self.print_long(i64::from(n));
    }

    /// pdfTeX §65: `print_int` of a `longinteger`.
    pub(crate) fn print_long(&mut self, mut n: i64) {
        let mut k = 0;
        if n < 0 {
            self.print_char(b'-');
            if n > -100_000_000 {
                n = -n;
            } else {
                let mut m = -1 - n;
                n = m / 10;
                m = (m % 10) + 1;
                k = 1;
                if m < 10 {
                    self.dig[0] = digit(m);
                } else {
                    self.dig[0] = 0;
                    n += 1;
                }
            }
        }
        loop {
            self.dig[k] = digit(n % 10);
            n /= 10;
            k += 1;
            if n == 0 {
                break;
            }
        }
        self.print_the_digs(k);
    }

    /// §66: print two least significant digits.
    pub(crate) fn print_two(&mut self, n: i32) {
        let n = n.unsigned_abs() % 100;
        self.print_char(b'0' + u8::try_from(n / 10).unwrap_or(0));
        self.print_char(b'0' + u8::try_from(n % 10).unwrap_or(0));
    }

    /// §67: print a nonnegative integer in hexadecimal.
    pub(crate) fn print_hex(&mut self, mut n: i32) {
        let mut k = 0;
        self.print_char(b'"');
        loop {
            self.dig[k] = digit(i64::from(n % 16));
            n /= 16;
            k += 1;
            if n == 0 {
                break;
            }
        }
        self.print_the_digs(k);
    }

    /// §69: print a positive integer in lowercase roman numerals.
    pub(crate) fn print_roman_int(&mut self, mut n: i32) {
        const S: &[u8] = b"m2d5c2l5x2v5i";
        let mut j = 0;
        let mut v = 1000;
        loop {
            while n >= v {
                self.print_char(S[j]);
                n -= v;
            }
            if n <= 0 {
                return; // nonpositive input produces no output
            }
            let mut k = j + 2;
            let mut u = v / i32::from(S[k - 1] - b'0');
            if S[k - 1] == b'2' {
                k += 2;
                u /= i32::from(S[k - 1] - b'0');
            }
            if n + u >= v {
                self.print_char(S[k]);
                n += u;
            } else {
                j += 2;
                v /= i32::from(S[j - 1] - b'0');
            }
        }
    }

    /// §70: print the string being built.
    pub(crate) fn print_current_string(&mut self) {
        for j in self.str_start[self.str_ptr]..self.pool_ptr {
            self.print_char(self.str_pool[j]);
        }
    }
}

/// Selector 0–15 as a `\write` stream index.
fn stream(selector: i32) -> usize {
    usize::try_from(selector).unwrap_or(0) & 15
}

/// A digit 0–15 for `dig`.
fn digit(d: i64) -> u8 {
    u8::try_from(d).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crate::testing::{engine, term_output};

    #[test]
    fn integers() {
        let mut t = engine();
        let out = term_output(&mut t, |t| {
            for n in [
                0,
                7,
                -7,
                123_456_789,
                -99_999_999,
                -100_000_000,
                i32::MIN,
                i32::MAX,
            ] {
                t.print_int(n);
                t.print_char(b' ');
            }
        });
        assert_eq!(
            out,
            b"0 7 -7 123456789 -99999999 -100000000 -2147483648 2147483647 "
        );
    }

    #[test]
    fn hex_two_roman() {
        let mut t = engine();
        let out = term_output(&mut t, |t| {
            t.print_hex(0);
            t.print_hex(0xBEEF);
            t.print_char(b' ');
            t.print_two(-5);
            t.print_two(1234);
            t.print_char(b' ');
            for n in [1990, 4, 9, 14, 49, 3999, 0] {
                t.print_roman_int(n);
                t.print_char(b',');
            }
        });
        assert_eq!(out, b"\"0\"BEEF 0534 mcmxc,iv,ix,xiv,xlix,mmmcmxcix,,");
    }

    #[test]
    fn unprintable_characters_use_caret_notation() {
        let mut t = engine();
        // INITEX starts with \newlinechar=0, which would turn ^^@ into a
        // line break; disable it.
        t.set_int_par(crate::eqtb::NEW_LINE_CHAR_CODE, -1);
        let out = term_output(&mut t, |t| {
            for c in [0, 9, 13, 65, 127, 128, 255] {
                t.print(c);
            }
        });
        assert_eq!(out, b"^^@^^I^^MA^^?^^80^^ff");
    }

    #[test]
    fn new_line_char_and_line_wrapping() {
        let mut t = engine();
        let out = term_output(&mut t, |t| {
            t.set_int_par(crate::eqtb::NEW_LINE_CHAR_CODE, i32::from(b'|'));
            t.print(i32::from(b'|'));
            t.print_char(b'x');
            t.print_ln();
            // max_print_line = 72 (trip): the 73rd character starts a new line.
            for _ in 0..73 {
                t.print_char(b'a');
            }
        });
        let mut want = b"\nx\n".to_vec();
        want.extend_from_slice(&[b'a'; 72]);
        want.extend_from_slice(b"\na");
        assert_eq!(out, want);
    }

    #[test]
    fn print_nl_and_esc() {
        let mut t = engine();
        let out = term_output(&mut t, |t| {
            t.print_nl(b"first");
            t.print_nl(b"second");
            t.print_esc(b"par");
            t.set_int_par(crate::eqtb::ESCAPE_CHAR_CODE, -1);
            t.print_esc(b"relax");
        });
        assert_eq!(out, b"first\nsecond\\parrelax");
    }
}
