//! The scalar rows of DESIGN §7.17.12 behind their accessors: each is a
//! scalar slot of the tables' convention, its version its value, made
//! at the write; a read tells the tracker the version it holds. A plain
//! run's tracker has no values, and each accessor is a field access.

use crate::host::Host;
use crate::tex::Tex;
use crate::track::{Row, Tracker, scalar};

/// A scalar row's value as the `i32` its version is made from.
pub(crate) trait ScalarBits: Copy {
    fn bits(self) -> i32;
}

impl ScalarBits for i32 {
    #[inline]
    fn bits(self) -> i32 {
        self
    }
}

impl ScalarBits for bool {
    #[inline]
    fn bits(self) -> i32 {
        i32::from(self)
    }
}

macro_rules! scalar_rows {
    ($( $get:ident, $set:ident, $field:ident, $slot:expr, $ty:ty );* $(;)?) => {
        impl<H: Host, T: Tracker> Tex<H, T> {
            $(
                #[doc = concat!("`", stringify!($field), "`, read.")]
                #[inline]
                pub(crate) fn $get(&self) -> $ty {
                    self.scalar_read(Row::Scalar($slot), ScalarBits::bits(self.$field));
                    self.$field
                }
                #[doc = concat!("`", stringify!($field), "`, written.")]
                #[inline]
                pub(crate) fn $set(&mut self, v: $ty) {
                    self.$field = v;
                    self.scalar_wrote(Row::Scalar($slot), ScalarBits::bits(v));
                }
            )*
        }
    };
}

scalar_rows! {
    selector, set_selector, selector, scalar::SELECTOR, i32;
    interaction, set_interaction, interaction, scalar::INTERACTION, i32;
    history, set_history, history, scalar::HISTORY, i32;
    error_count, set_error_count, error_count, scalar::ERROR_COUNT, i32;
    shown_mode, set_shown_mode, shown_mode, scalar::SHOWN_MODE, i32;
    last_badness, set_last_badness, last_badness, scalar::LAST_BADNESS, i32;
    mag_set, set_mag_set, mag_set, scalar::MAG_SET, i32;
    align_state, set_align_state, align_state, scalar::ALIGN_STATE, i32;
    dead_cycles, set_dead_cycles, dead_cycles, scalar::DEAD_CYCLES, i32;
    after_token, set_after_token, after_token, scalar::AFTER_TOKEN, i32;
    long_help_seen, set_long_help_seen, long_help_seen, scalar::LONG_HELP_SEEN, bool;
    log_opened, set_log_opened, log_opened, scalar::LOG_OPENED, bool;
    open_parens, set_open_parens, open_parens, scalar::OPEN_PARENS, i32;
    synctex_tags, set_synctex_tags, synctex_tags, scalar::SYNCTEX_TAGS, i32;
    synctex_flags, set_synctex_flags, synctex_flags, scalar::SYNCTEX_FLAGS, i32;
    sys_time, set_sys_time, sys_time, scalar::SYS_TIME, i32;
    sys_day, set_sys_day, sys_day, scalar::SYS_DAY, i32;
    sys_month, set_sys_month, sys_month, scalar::SYS_MONTH, i32;
    sys_year, set_sys_year, sys_year, scalar::SYS_YEAR, i32;
}

/// The rows that hold a string's number, versioned by its characters: a
/// step that runs again makes the name again, a new string with the same
/// characters (the string pool is not put back, DESIGN 7.17.3), and the
/// steps that read it are not woken by its number.
macro_rules! name_rows {
    ($( $get:ident, $set:ident, $field:ident, $slot:expr );* $(;)?) => {
        impl<H: Host, T: Tracker> Tex<H, T> {
            $(
                #[doc = concat!("`", stringify!($field), "`, read.")]
                #[inline]
                pub(crate) fn $get(&self) -> i32 {
                    if T::VALUES {
                        self.tracker
                            .row_read(Row::Scalar($slot), || self.name_version(self.$field));
                    }
                    self.$field
                }
                #[doc = concat!("`", stringify!($field), "`, written.")]
                #[inline]
                pub(crate) fn $set(&mut self, v: i32) {
                    self.$field = v;
                    if T::VALUES {
                        self.tracker.row_wrote(Row::Scalar($slot), self.name_version(v));
                    }
                }
            )*
        }
    };
}

name_rows! {
    job_name, set_job_name, job_name, scalar::JOB_NAME;
    log_name, set_log_name, log_name, scalar::LOG_NAME;
    output_file_name, set_output_file_name, output_file_name, scalar::OUTPUT_FILE_NAME;
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// String `s`'s version as a name row holds it: its characters (odd,
    /// apart from the even versions of numbers), or its number if it is
    /// none or a character.
    fn name_version(&self, s: i32) -> u128 {
        match usize::try_from(s) {
            Ok(n) if n >= 256 && n < self.str_ptr => self.string_version(n) | 1,
            _ => crate::track::scalar_version_i32(s),
        }
    }

    /// `\pdfelapsedtime`'s start (seconds, microseconds), read.
    #[inline]
    pub(crate) fn epoch(&self) -> (i32, i32) {
        self.scalar_read(Row::Scalar(scalar::EPOCH_S), self.epoch.0);
        self.scalar_read(Row::Scalar(scalar::EPOCH_US), self.epoch.1);
        self.epoch
    }

    /// `\pdfelapsedtime`'s start, written.
    #[inline]
    pub(crate) fn set_epoch(&mut self, v: (i32, i32)) {
        self.epoch = v;
        self.scalar_wrote(Row::Scalar(scalar::EPOCH_S), v.0);
        self.scalar_wrote(Row::Scalar(scalar::EPOCH_US), v.1);
    }

    /// `write_open[j]` (§1342), read.
    #[inline]
    pub(crate) fn write_open(&self, j: usize) -> bool {
        let v = self.write_open[j];
        self.scalar_read(Row::Scalar(write_open_slot(j)), i32::from(v));
        v
    }

    /// `write_open[j]`, written.
    #[inline]
    pub(crate) fn set_write_open(&mut self, j: usize, v: bool) {
        self.write_open[j] = v;
        self.scalar_wrote(Row::Scalar(write_open_slot(j)), i32::from(v));
    }

    /// `read_open[j]` (§480), read.
    #[inline]
    pub(crate) fn read_open(&self, j: usize) -> i32 {
        let v = self.read_open[j];
        self.scalar_read(Row::Scalar(read_open_slot(j)), v);
        v
    }

    /// `read_open[j]`, written.
    #[inline]
    pub(crate) fn set_read_open(&mut self, j: usize, v: i32) {
        self.read_open[j] = v;
        self.scalar_wrote(Row::Scalar(read_open_slot(j)), v);
    }

    /// Every scalar row's version, made from the fields as they are (a
    /// writer that stores them past their accessors: the engine as made,
    /// a format's load).
    pub(crate) fn version_scalars(&self) {
        if !T::VALUES {
            return;
        }
        let rows: [(u16, i32); 26] = [
            (
                scalar::GLUE_LINEAGE,
                i32::try_from(self.glue_lineage).unwrap_or(i32::MAX),
            ),
            (scalar::FONT_COUNT, self.fonts.count),
            (scalar::LAST_BADNESS, self.last_badness),
            (scalar::OUTPUT_ACTIVE, i32::from(self.output_active)),
            (scalar::TERM_OFFSET, self.term_offset),
            (scalar::FILE_OFFSET, self.file_offset),
            (scalar::SELECTOR, self.selector),
            (scalar::INTERACTION, self.interaction),
            (scalar::HISTORY, self.history),
            (scalar::ERROR_COUNT, self.error_count),
            (scalar::SHOWN_MODE, self.shown_mode),
            (scalar::MAG_SET, self.mag_set),
            (scalar::ALIGN_STATE, self.align_state),
            (scalar::DEAD_CYCLES, self.dead_cycles),
            (scalar::AFTER_TOKEN, self.after_token),
            (scalar::LONG_HELP_SEEN, i32::from(self.long_help_seen)),
            (scalar::LOG_OPENED, i32::from(self.log_opened)),
            (scalar::OPEN_PARENS, self.open_parens),
            (scalar::SYS_TIME, self.sys_time),
            (scalar::SYS_DAY, self.sys_day),
            (scalar::SYS_MONTH, self.sys_month),
            (scalar::SYS_YEAR, self.sys_year),
            (scalar::EPOCH_S, self.epoch.0),
            (scalar::EPOCH_US, self.epoch.1),
            (scalar::HASH_HIGH, self.hash_high),
            (scalar::HASH_USED, self.hash_used),
        ];
        for (k, v) in rows {
            self.tracker
                .row_made(Row::Scalar(k), crate::track::scalar_version_i32(v));
        }
        for (k, s) in [
            (scalar::JOB_NAME, self.job_name),
            (scalar::LOG_NAME, self.log_name),
            (scalar::OUTPUT_FILE_NAME, self.output_file_name),
        ] {
            self.tracker.row_made(Row::Scalar(k), self.name_version(s));
        }
        self.tracker.row_made(
            Row::Scalar(scalar::STR_TOP),
            crate::track::scalar_version(self.str_ptr),
        );
        for (j, &v) in self.write_open.iter().enumerate() {
            self.tracker.row_made(
                Row::Scalar(write_open_slot(j)),
                crate::track::scalar_version_i32(i32::from(v)),
            );
        }
        for (j, &v) in self.read_open.iter().enumerate() {
            self.tracker.row_made(
                Row::Scalar(read_open_slot(j)),
                crate::track::scalar_version_i32(v),
            );
        }
    }
}

fn write_open_slot(j: usize) -> u16 {
    scalar::WRITE_OPEN + u16::try_from(j.min(31)).unwrap_or(31)
}

fn read_open_slot(j: usize) -> u16 {
    scalar::READ_OPEN + u16::try_from(j.min(31)).unwrap_or(31)
}
