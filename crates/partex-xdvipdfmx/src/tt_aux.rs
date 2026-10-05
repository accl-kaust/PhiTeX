//! tt_aux.c, tt_aux.h: TrueType Collection offsets and the font
//! descriptor of an sfnt.

use crate::fmt::round_acc;
use crate::prelude::*;
use crate::sfnt::{SFNT_TYPE_TTC, Sfnt, ULONG, fixed};

/// Font descriptor `/Flags` bits (tt_aux.c).
pub const FIXEDWIDTH: i32 = 1 << 0;
pub const SERIF: i32 = 1 << 1;
pub const SYMBOLIC: i32 = 1 << 2;
pub const SCRIPT: i32 = 1 << 3;
pub const STANDARD: i32 = 1 << 5;
pub const ITALIC: i32 = 1 << 6;
pub const ALLCAP: i32 = 1 << 16;
pub const SMALLCAP: i32 = 1 << 17;
pub const FORCEBOLD: i32 = 1 << 18;

impl Sfnt {
    /// `ttc_read_offset`: the offset of the `ttc_idx`th font of a TTC.
    pub fn ttc_read_offset(&mut self, ttc_idx: ULONG) -> ULONG {
        if self.type_ != SFNT_TYPE_TTC {
            error!("ttc_read_offset(): invalid font type");
        }

        self.sfnt_seek_set(4); /* skip version tag */

        let _version = self.sfnt_get_ulong();
        let num_dirs = self.sfnt_get_ulong();
        if ttc_idx > num_dirs.wrapping_sub(1) {
            error!("Invalid TTC index number");
        }

        self.sfnt_seek_set(12u32.wrapping_add(ttc_idx.wrapping_mul(4)));
        self.sfnt_get_ulong()
    }
}

impl Dpx {
    /// `tt_get_fontdesc`: the FontDescriptor dict (none without a `post`
    /// table); `embed` is in/out (cleared when the license forbids
    /// embedding, unless `ignore_font_license`). `type_` is C's `type`: 0
    /// for a CID-keyed font (adds /Style /Panose), 1 for a simple font.
    pub fn tt_get_fontdesc(
        &mut self,
        sfont: &mut Sfnt,
        embed: &mut i32,
        stemv: i32,
        type_: i32,
        fontname: &[u8],
    ) -> Option<Obj> {
        let mut stemv = stemv;
        let mut flag = SYMBOLIC;

        let os2 = sfont.tt_read_os2__table();
        let head = sfont.tt_read_head_table();
        let post = sfont.tt_read_post_table()?;

        let upem = f64::from(head.units_per_em);
        let pdfunit = |v: f64| round_acc((1000.0 * v) / upem, 1.0);

        let descriptor = self.o.new_dict();
        self.o.put_name(descriptor, b"Type", b"FontDescriptor");

        if *embed != 0 {
            // License: the least restrictive license granted takes
            // precedence; "Preview & Print" embedding is allowed.
            if os2.fs_type == 0x0000 || (os2.fs_type & 0x0008) != 0 {
                *embed = 1;
            } else if (os2.fs_type & 0x0004) != 0 {
                if self.conf.verbose_level > 0 {
                    warn!(
                        "Font \"{}\" permits \"Preview & Print\" embedding only **\n",
                        alloc::string::String::from_utf8_lossy(fontname)
                    );
                }
                *embed = 1;
            } else if self.conf.ignore_font_license {
                *embed = 1;
            } else {
                *embed = 0;
            }
        }

        self.o.put_number(
            descriptor,
            b"Ascent",
            pdfunit(f64::from(os2.s_typo_ascender)),
        );
        self.o.put_number(
            descriptor,
            b"Descent",
            pdfunit(f64::from(os2.s_typo_descender)),
        );
        if stemv < 0 {
            // Not given by the option '-v'.
            let w = f64::from(os2.us_weight_class) / 65.0;
            stemv = (w * w + 50.0) as i32;
        }
        self.o.put_number(descriptor, b"StemV", f64::from(stemv));
        if os2.version == 0x0002 {
            self.o.put_number(
                descriptor,
                b"CapHeight",
                pdfunit(f64::from(os2.s_cap_height)),
            );
            // Optional.
            self.o
                .put_number(descriptor, b"XHeight", pdfunit(f64::from(os2.sx_height)));
        } else {
            // Arbitrary.
            self.o.put_number(
                descriptor,
                b"CapHeight",
                pdfunit(f64::from(os2.s_typo_ascender)),
            );
        }
        // Optional.
        if os2.x_avg_char_width != 0 {
            self.o.put_number(
                descriptor,
                b"AvgWidth",
                pdfunit(f64::from(os2.x_avg_char_width)),
            );
        }

        // BoundingBox (array).
        let bbox = self.o.new_array();
        for v in [head.x_min, head.y_min, head.x_max, head.y_max] {
            let n = self.o.new_number(pdfunit(f64::from(v)));
            self.o.add_array(bbox, n);
        }
        self.o.put(descriptor, b"FontBBox", bbox);

        // post
        self.o
            .put_number(descriptor, b"ItalicAngle", fixed(post.italic_angle));

        // Flags.
        if os2.fs_selection & (1 << 0) != 0 {
            flag |= ITALIC;
        }
        if os2.fs_selection & (1 << 5) != 0 {
            flag |= FORCEBOLD;
        }
        if ((i32::from(os2.s_family_class) >> 8) & 0xff) != 8 {
            flag |= SERIF;
        }
        if ((i32::from(os2.s_family_class) >> 8) & 0xff) == 10 {
            flag |= SCRIPT;
        }
        if post.is_fixed_pitch != 0 {
            flag |= FIXEDWIDTH;
        }

        self.o.put_number(descriptor, b"Flags", f64::from(flag));

        // Insert panose if you want.
        if type_ == 0 {
            // CID-keyed font: add panose.
            let mut panose = [0u8; 12];
            panose[0] = (i32::from(os2.s_family_class) >> 8) as u8;
            panose[1] = (i32::from(os2.s_family_class) & 0xff) as u8;
            panose[2..].copy_from_slice(&os2.panose);

            let styledict = self.o.new_dict();
            self.o.put_string(styledict, b"Panose", &panose);
            self.o.put(descriptor, b"Style", styledict);
        }

        Some(descriptor)
    }
}
