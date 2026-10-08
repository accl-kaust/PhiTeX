//! A slot's content version now: the same function each accessor's
//! tracker hook gives the version a read sees (DESIGN 3.17.1), so that a
//! definition's version, made at a step's end, and a later read's, made
//! by the hook, agree.

use crate::host::Host;
use crate::ssa::{Fam, Slot};
use crate::tex::Tex;
use crate::track::{Tracker, list, save, scalar, scalar_version, scalar_version_i32};
use partex_ssa::Version;

/// Slot `s`'s version in `t` now, if this layer versions its family.
pub(crate) fn slot_version<H: Host, T: Tracker>(t: &Tex<H, T>, s: Slot) -> Option<u128> {
    let i32of = |x: i64| i32::try_from(x).unwrap_or(0);
    Some(match s.0 {
        Fam::Eqtb => t.eqtb_content_of(i32of(s.1), false),
        Fam::List => {
            let i = u32::try_from(s.1).ok()?;
            let (d, f) = (i / list::STRIDE, u8::try_from(i % list::STRIDE).ok()?);
            if d == 0 && f == list::COUNT {
                t.nest_version()
            } else if d == 0 && f > list::COUNT {
                t.align.field_version(f - list::COUNT - 1).0
            } else {
                let l = t.level_at_depth(usize::try_from(d).ok()?)?;
                crate::nest::field_version(l, f)
            }
        }
        Fam::Save => {
            let k = u32::try_from(s.1).ok()?;
            if (save::ENTRY..save::XENTRY).contains(&k) {
                t.save_entry_version(i32::try_from(k - save::ENTRY).ok()?)
            } else {
                t.save_row_version(s.1)
            }
        }
        Fam::Cond => t.cond_version(),
        Fam::Mark => {
            let k = u32::try_from(s.1).ok()?;
            let (c, m) = ((k / 5).cast_signed(), (k % 5).cast_signed());
            t.mark_version(c, m)
        }
        Fam::Page => t.page_field_version(u8::try_from(s.1).ok()?),
        Fam::Font => {
            let k = u32::try_from(s.1).ok()?;
            let fields = crate::track::font::FIELDS;
            t.fonts
                .field_version(i32::try_from(k / fields).ok()?, k % fields)
        }
        Fam::FontTable => t.fonts.table_version(t.font_ptr, t.fmem_ptr),
        Fam::Hyph => match u32::try_from(s.1).ok()? {
            crate::track::hyph::PATTERNS => t.hyph.pat_ver,
            _ => t.hyph.exceptions_version(),
        },
        Fam::Pdf => t.writer_content(u8::try_from(s.1).ok()?),
        Fam::Dvi => t.writer_content(u8::try_from(s.1).ok()? + crate::pdf::val::DVI),
        Fam::Out => t.out_version(u8::try_from(s.1).ok()?),
        Fam::Read => t.read_version(usize::try_from(s.1).ok()?),
        Fam::Random => Version::of(&t.random).0,
        Fam::Alloc => {
            let k = u16::try_from(s.1).ok()?;
            match k {
                scalar::JOB_NAME => t.name_version(t.job_name),
                scalar::LOG_NAME => t.name_version(t.log_name),
                scalar::OUTPUT_FILE_NAME => t.name_version(t.output_file_name),
                scalar::STR_TOP => scalar_version(t.str_ptr),
                k => scalar_version_i32(crate::ssa::scalar_get(t, k)?),
            }
        }
        Fam::Sealed => t
            .seals
            .get(u128::from(s.1.cast_unsigned()))
            .map_or(Version::ABSENT.0, |x| x.version()),
        _ => return None,
    })
}
