//! sfnt.c, sfnt.h: TrueType/OpenType (sfnt) font files.
//!
//! An [`Sfnt`] owns its file (C's `FILE *stream`; the caller's `fp` is
//! moved in, `sfnt_close` is `Drop`). Every C function whose first
//! parameter is `sfnt *` and that touches no global is an `impl Sfnt`
//! method with the C name (also in tt_table.rs, tt_post.rs, tt_glyf.rs,
//! tt_cmap.rs, tt_aux.rs, tt_gsub.rs); one that makes PDF objects or reads
//! other global state is an `impl Dpx` method taking `sfont: &mut Sfnt`.
//! A CFF in an OpenType file is read from `sfont.stream.clone()` (shares
//! the bytes).

use crate::prelude::*;

pub type BYTE = u8;
pub type CHAR = i8;
pub type USHORT = u16;
pub type SHORT = i16;
pub type ULONG = u32;
pub type LONG = i32;
/// 16.16 fixed point (`uint32_t` in C).
pub type Fixed = u32;
pub type FWord = i16;
/// `uFWord`.
pub type UFWord = u16;

/// `SFNT_TABLE_REQUIRED`.
pub const SFNT_TABLE_REQUIRED: u8 = 1 << 0;

/// `SFNT_TYPE_TRUETYPE`.
pub const SFNT_TYPE_TRUETYPE: i32 = 1 << 0;
/// `SFNT_TYPE_OPENTYPE`.
pub const SFNT_TYPE_OPENTYPE: i32 = 1 << 1;
/// `SFNT_TYPE_POSTSCRIPT`.
pub const SFNT_TYPE_POSTSCRIPT: i32 = 1 << 2;
/// `SFNT_TYPE_TTC`.
pub const SFNT_TYPE_TTC: i32 = 1 << 4;
/// `SFNT_TYPE_DFONT`.
pub const SFNT_TYPE_DFONT: i32 = 1 << 8;

/// `SFNT_TRUETYPE` (sfnt.c): the version of a TrueType file.
pub const SFNT_TRUETYPE: u32 = 0x0001_0000;
/// `SFNT_MAC_TRUE` (`true`).
pub const SFNT_MAC_TRUE: u32 = 0x7472_7565;
/// `SFNT_OPENTYPE`.
pub const SFNT_OPENTYPE: u32 = 0x0001_0000;
/// `SFNT_POSTSCRIPT` (`OTTO`).
pub const SFNT_POSTSCRIPT: u32 = 0x4f54_544f;
/// `SFNT_TTC` (`ttcf`).
pub const SFNT_TTC: u32 = 0x7474_6366;

/// `struct sfnt_table`.
#[derive(Clone, Debug, Default)]
pub struct SfntTable {
    pub tag: [u8; 4],
    pub check_sum: ULONG,
    pub offset: ULONG,
    pub length: ULONG,
    /// The table's data when set by `sfnt_set_table` (C's `data`; none:
    /// read from the file at `offset`).
    pub data: Option<Vec<u8>>,
}

/// `struct sfnt_table_directory`.
#[derive(Clone, Debug, Default)]
pub struct SfntTableDirectory {
    /// Fixed for Win.
    pub version: ULONG,
    pub num_tables: USHORT,
    pub search_range: USHORT,
    pub entry_selector: USHORT,
    pub range_shift: USHORT,
    /// Number of kept tables.
    pub num_kept_tables: USHORT,
    /// Keep or omit (`SFNT_TABLE_REQUIRED`), one per table.
    pub flags: Vec<u8>,
    pub tables: Vec<SfntTable>,
}

/// `sfnt`.
#[derive(Clone, Debug)]
pub struct Sfnt {
    /// `SFNT_TYPE_*` (C's `type`).
    pub type_: i32,
    pub directory: Option<Box<SfntTableDirectory>>,
    pub stream: MemFile,
    pub offset: ULONG,
}

/// The `fixed(a)` macro: a 16.16 number as a double.
#[must_use]
pub fn fixed(a: Fixed) -> f64 {
    // C: (double)(a % 0x10000L) / (double)0x10000L + a / 0x10000L
    //    - ((a / 0x10000L > 0x7fffL) ? 0x10000L : 0), `a` widened to long.
    let a = i64::from(a);
    let hi = a / 0x10000;
    (a % 0x10000) as f64 / 65536.0 + hi as f64 - if hi > 0x7fff { 65536.0 } else { 0.0 }
}

/// `put_big_endian`: `q` as `n` bytes at the start of `s`; returns `n`.
pub fn put_big_endian(s: &mut [u8], q: LONG, n: i32) -> i32 {
    let mut q = q;
    let mut i = n - 1;
    while i >= 0 {
        s[i as usize] = (q & 0xff) as u8;
        q >>= 8;
        i -= 1;
    }
    n
}

/// `sfnt_put_ushort` macro: 2.
pub fn sfnt_put_ushort(s: &mut [u8], v: USHORT) -> i32 {
    put_big_endian(s, LONG::from(v), 2)
}

/// `sfnt_put_short` macro: 2.
pub fn sfnt_put_short(s: &mut [u8], v: SHORT) -> i32 {
    put_big_endian(s, LONG::from(v), 2)
}

/// `sfnt_put_ulong` macro: 4.
pub fn sfnt_put_ulong(s: &mut [u8], v: ULONG) -> i32 {
    put_big_endian(s, v as LONG, 4)
}

/// `sfnt_put_long` macro: 4.
pub fn sfnt_put_long(s: &mut [u8], v: LONG) -> i32 {
    put_big_endian(s, v, 4)
}

/// `convert_tag` (static): a 4-byte tag from a big-endian number.
fn convert_tag(u_tag: u32) -> [u8; 4] {
    let mut tag = [0u8; 4];
    let mut u_tag = u_tag;
    for i in (0..4).rev() {
        tag[i] = (u_tag % 256) as u8;
        u_tag /= 256;
    }
    tag
}

/// `max2floor` (static): the largest power of two `<= n`.
fn max2floor(n: u32) -> u32 {
    let mut n = n;
    let mut val: i32 = 1;
    while n > 1 {
        n /= 2;
        val = val.wrapping_mul(2);
    }
    val as u32
}

/// `log2floor` (static).
fn log2floor(n: u32) -> u32 {
    let mut n = n;
    let mut val = 0u32;
    while n > 1 {
        n /= 2;
        val += 1;
    }
    val
}

/// `sfnt_calc_checksum` (static).
fn sfnt_calc_checksum(data: &[u8]) -> ULONG {
    let mut chksum: ULONG = 0;
    let mut count = 0;
    for &b in data {
        chksum = chksum.wrapping_add(u32::from(b) << (8 * (3 - count)));
        count = (count + 1) & 3;
    }
    chksum
}

/// `find_table_index` (static): the index of `tag`, or -1.
fn find_table_index(td: Option<&SfntTableDirectory>, tag: &[u8]) -> i32 {
    let Some(td) = td else {
        return -1;
    };
    for idx in 0..td.num_tables as usize {
        if td.tables[idx].tag[..] == tag[..4] {
            return idx as i32;
        }
    }
    -1
}

impl Sfnt {
    /// `sfnt_open`: takes the file over. C never returns NULL: a file that
    /// is no sfnt gets type 0.
    pub fn sfnt_open(fp: MemFile) -> Option<Sfnt> {
        let mut sfont = Sfnt {
            type_: 0,
            directory: None,
            stream: fp,
            offset: 0,
        };
        sfont.stream.rewind();
        let type_ = sfont.sfnt_get_ulong();
        if type_ == SFNT_TRUETYPE || type_ == SFNT_MAC_TRUE {
            sfont.type_ = SFNT_TYPE_TRUETYPE;
        } else if type_ == SFNT_OPENTYPE {
            sfont.type_ = SFNT_TYPE_OPENTYPE;
        } else if type_ == SFNT_POSTSCRIPT {
            sfont.type_ = SFNT_TYPE_POSTSCRIPT;
        } else if type_ == SFNT_TTC {
            sfont.type_ = SFNT_TYPE_TTC;
        }
        sfont.stream.rewind();
        sfont.directory = None;
        sfont.offset = 0;
        Some(sfont)
    }
    /// `dfont_open`: the `index`th sfnt resource of a Mac dfont; none if
    /// it has no `sfnt` resource.
    pub fn dfont_open(fp: MemFile, index: i32) -> Option<Sfnt> {
        let mut sfont = Sfnt {
            type_: 0,
            directory: None,
            stream: fp,
            offset: 0,
        };
        sfont.stream.rewind();
        let rdata_pos = sfont.sfnt_get_ulong();
        let map_pos = sfont.sfnt_get_ulong();
        sfont.sfnt_seek_set(map_pos.wrapping_add(0x18));
        let tags_pos = map_pos.wrapping_add(ULONG::from(sfont.sfnt_get_ushort()));
        sfont.sfnt_seek_set(tags_pos);
        let tags_num = sfont.sfnt_get_ushort();

        let mut types_num: USHORT = 0;
        let mut types_pos: ULONG = 0;
        // C's `i` is a USHORT: `i <= tags_num` never ends for 0xffff.
        let mut i: USHORT = 0;
        loop {
            if i > tags_num {
                break;
            }
            let tag = sfont.sfnt_get_ulong(); /* tag name */
            types_num = sfont.sfnt_get_ushort(); /* typefaces number */
            types_pos = tags_pos.wrapping_add(ULONG::from(sfont.sfnt_get_ushort()));
            if tag == 0x7366_6e74 {
                /* "sfnt" */
                break;
            }
            i = i.wrapping_add(1);
        }
        if i > tags_num {
            return None;
        }

        sfont.sfnt_seek_set(types_pos);
        if index > i32::from(types_num) {
            error!("Invalid index {} for dfont.", index);
        }

        let mut res_pos: ULONG = 0;
        let mut i: USHORT = 0;
        loop {
            if i > types_num {
                break;
            }
            let _ = sfont.sfnt_get_ushort(); /* resource id */
            let _ = sfont.sfnt_get_ushort(); /* resource name position */
            res_pos = sfont.sfnt_get_ulong(); /* resource flag + offset */
            sfont.sfnt_get_ulong(); /* mbz */
            if i32::from(i) == index {
                break;
            }
            i = i.wrapping_add(1);
        }

        sfont.stream.rewind();
        sfont.type_ = SFNT_TYPE_DFONT;
        sfont.directory = None;
        sfont.offset = (res_pos & 0x00ff_ffff)
            .wrapping_add(rdata_pos)
            .wrapping_add(4);
        Some(sfont)
    }
    /// `sfnt_close`.
    pub fn sfnt_close(self) {}

    /// `sfnt_get_byte`.
    pub fn sfnt_get_byte(&mut self) -> BYTE {
        self.stream.get_unsigned_byte()
    }
    /// `sfnt_get_char`.
    pub fn sfnt_get_char(&mut self) -> CHAR {
        self.stream.get_signed_byte()
    }
    /// `sfnt_get_ushort`.
    pub fn sfnt_get_ushort(&mut self) -> USHORT {
        self.stream.get_unsigned_pair()
    }
    /// `sfnt_get_short`.
    pub fn sfnt_get_short(&mut self) -> SHORT {
        self.stream.get_signed_pair()
    }
    /// `sfnt_get_ulong`.
    pub fn sfnt_get_ulong(&mut self) -> ULONG {
        self.stream.get_unsigned_quad()
    }
    /// `sfnt_get_long`.
    pub fn sfnt_get_long(&mut self) -> LONG {
        self.stream.get_signed_quad()
    }
    /// `sfnt_get_uint24`.
    pub fn sfnt_get_uint24(&mut self) -> ULONG {
        self.stream.get_unsigned_triple()
    }
    /// `sfnt_seek_set`.
    pub fn sfnt_seek_set(&mut self, o: ULONG) {
        self.stream.seek_absolute(o as usize);
    }
    /// `sfnt_read(b, l, s)`: fills `buf` (`l` = its length); bytes read.
    pub fn sfnt_read(&mut self, buf: &mut [u8]) -> usize {
        self.stream.read_into(buf)
    }

    /// `sfnt_read_table_directory`: 0, or -1 on error.
    pub fn sfnt_read_table_directory(&mut self, offset: ULONG) -> i32 {
        self.directory = None;
        self.sfnt_seek_set(offset);
        let mut td = SfntTableDirectory {
            version: self.sfnt_get_ulong(),
            num_tables: self.sfnt_get_ushort(),
            search_range: self.sfnt_get_ushort(),
            entry_selector: self.sfnt_get_ushort(),
            range_shift: self.sfnt_get_ushort(),
            ..SfntTableDirectory::default()
        };
        td.flags = vec![0; td.num_tables as usize];
        td.tables = Vec::with_capacity(td.num_tables as usize);
        for _ in 0..td.num_tables {
            let u_tag = self.sfnt_get_ulong();
            let tag = convert_tag(u_tag);
            let check_sum = self.sfnt_get_ulong();
            let offset = self.sfnt_get_ulong().wrapping_add(self.offset);
            let length = self.sfnt_get_ulong();
            td.tables.push(SfntTable {
                tag,
                check_sum,
                offset,
                length,
                data: None,
            });
        }
        td.num_kept_tables = 0;
        self.directory = Some(Box::new(td));
        0
    }
    /// `sfnt_find_table_len`: 0 if absent.
    pub fn sfnt_find_table_len(&self, tag: &[u8]) -> ULONG {
        let td = self.directory.as_deref();
        let idx = find_table_index(td, tag);
        if idx < 0 {
            0
        } else {
            td.unwrap().tables[idx as usize].length
        }
    }
    /// `sfnt_find_table_pos`: 0 if absent.
    pub fn sfnt_find_table_pos(&self, tag: &[u8]) -> ULONG {
        let td = self.directory.as_deref();
        let idx = find_table_index(td, tag);
        if idx < 0 {
            0
        } else {
            td.unwrap().tables[idx as usize].offset
        }
    }
    /// `sfnt_locate_table`: seeks to the table and returns its offset
    /// (ERROR if absent).
    pub fn sfnt_locate_table(&mut self, tag: &[u8]) -> ULONG {
        let offset = self.sfnt_find_table_pos(tag);
        if offset == 0 {
            error!("sfnt: table not found...");
        }
        self.sfnt_seek_set(offset);
        offset
    }
    /// `sfnt_set_table`: `length` is `data.len()`. A new table also gets a
    /// flag (C does not grow `flags`, and reads past it).
    pub fn sfnt_set_table(&mut self, tag: &[u8], data: Vec<u8>) {
        let td = self
            .directory
            .as_deref_mut()
            .expect("sfnt_set_table: no table directory");
        let mut idx = find_table_index(Some(td), tag);
        if idx < 0 {
            idx = i32::from(td.num_tables);
            td.num_tables = td.num_tables.wrapping_add(1);
            let mut t = [0u8; 4];
            t.copy_from_slice(&tag[..4]);
            td.tables.push(SfntTable {
                tag: t,
                ..SfntTable::default()
            });
            td.flags.push(0);
        }
        let t = &mut td.tables[idx as usize];
        t.check_sum = sfnt_calc_checksum(&data);
        t.offset = 0;
        t.length = data.len() as ULONG;
        t.data = Some(data);
    }
    /// `sfnt_require_table`: 0, or -1 if a `must_exist` table is absent.
    pub fn sfnt_require_table(&mut self, tag: &[u8], must_exist: i32) -> i32 {
        let td = self
            .directory
            .as_deref_mut()
            .expect("sfnt_require_table: no table directory");
        let idx = find_table_index(Some(td), tag);
        if idx < 0 {
            if must_exist != 0 {
                return -1;
            }
        } else {
            td.flags[idx as usize] |= SFNT_TABLE_REQUIRED;
            td.num_kept_tables = td.num_kept_tables.wrapping_add(1);
        }
        0
    }
}

impl Dpx {
    /// `sfnt_create_FontFile_stream`: the tables kept, in directory order,
    /// each on a 4-byte boundary (zero padded); `/Length1` the total.
    pub fn sfnt_create_FontFile_stream(&mut self, sfont: &mut Sfnt) -> Option<Obj> {
        let stream = self.o.new_stream(crate::obj::STREAM_COMPRESS);
        let mut wbuf = [0u8; 1024];

        let (version, num_kept_tables, num_tables) = {
            let td = sfont
                .directory
                .as_deref()
                .expect("sfnt_create_FontFile_stream: no table directory");
            (td.version, td.num_kept_tables, td.num_tables as usize)
        };

        // Header.
        let mut p = 0usize;
        p += sfnt_put_ulong(&mut wbuf[p..], version) as usize;
        p += sfnt_put_ushort(&mut wbuf[p..], num_kept_tables) as usize;
        let sr = (max2floor(u32::from(num_kept_tables)) as i32).wrapping_mul(16);
        p += sfnt_put_ushort(&mut wbuf[p..], sr as USHORT) as usize;
        p += sfnt_put_ushort(
            &mut wbuf[p..],
            log2floor(u32::from(num_kept_tables)) as USHORT,
        ) as usize;
        sfnt_put_ushort(
            &mut wbuf[p..],
            (i32::from(num_kept_tables) * 16 - sr) as USHORT,
        );
        self.o.add_stream(stream, &wbuf[..12]);

        // Compute the start of the tables (after the headers).
        let mut offset: i32 = 12 + 16 * i32::from(num_kept_tables);
        for i in 0..num_tables {
            let td = sfont.directory.as_deref().unwrap();
            if td.flags[i] & SFNT_TABLE_REQUIRED != 0 {
                if offset % 4 != 0 {
                    offset += 4 - (offset % 4);
                }
                let t = &td.tables[i];
                wbuf[..4].copy_from_slice(&t.tag);
                let mut p = 4;
                p += sfnt_put_ulong(&mut wbuf[p..], t.check_sum) as usize;
                p += sfnt_put_ulong(&mut wbuf[p..], offset as ULONG) as usize;
                sfnt_put_ulong(&mut wbuf[p..], t.length);
                self.o.add_stream(stream, &wbuf[..16]);
                offset = offset.wrapping_add(t.length as i32);
            }
        }

        let padbytes = [0u8; 4];
        offset = 12 + 16 * i32::from(num_kept_tables);
        for i in 0..num_tables {
            let (flag, toff, tlen) = {
                let td = sfont.directory.as_deref().unwrap();
                (td.flags[i], td.tables[i].offset, td.tables[i].length)
            };
            if flag & SFNT_TABLE_REQUIRED == 0 {
                continue;
            }
            if offset % 4 != 0 {
                let length = 4 - (offset % 4);
                self.o.add_stream(stream, &padbytes[..length as usize]);
                offset += length;
            }
            let data = sfont.directory.as_deref_mut().unwrap().tables[i]
                .data
                .take();
            match data {
                None => {
                    let mut length = tlen as i32;
                    sfont.sfnt_seek_set(toff);
                    while length > 0 {
                        let n = (length as usize).min(1024);
                        let nb_read = sfont.sfnt_read(&mut wbuf[..n]);
                        if nb_read == 0 {
                            // C loops forever on a truncated file.
                            self.o.release(stream);
                            error!("Reading file failed...");
                        }
                        self.o.add_stream(stream, &wbuf[..nb_read]);
                        length -= nb_read as i32;
                    }
                }
                Some(d) => {
                    self.o.add_stream(stream, &d[..tlen as usize]);
                }
            }
            // Set offset for next table.
            offset = offset.wrapping_add(tlen as i32);
        }

        let stream_dict = self.o.stream_dict(stream);
        self.o
            .put_number(stream_dict, b"Length1", f64::from(offset));
        Some(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_values() {
        assert_eq!(fixed(0x0001_0000), 1.0);
        assert_eq!(fixed(0x0000_8000), 0.5);
        // -0.5 is 0xFFFF8000: 0.5 + 65535 - 65536.
        assert_eq!(fixed(0xFFFF_8000), -0.5);
        assert_eq!(fixed(0xFFF4_0000), -12.0);
    }

    #[test]
    fn checksum_and_floors() {
        assert_eq!(
            sfnt_calc_checksum(&[1, 2, 3, 4, 5]),
            0x0102_0304 + 0x0500_0000
        );
        assert_eq!(max2floor(11), 8);
        assert_eq!(log2floor(11), 3);
        assert_eq!(max2floor(0), 1);
        let mut b = [0u8; 4];
        sfnt_put_short(&mut b, -2);
        assert_eq!(&b[..2], &[0xff, 0xfe]);
    }
}
