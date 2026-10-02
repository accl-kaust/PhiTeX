//! Time as web2c tells it: the job's start (`\time`, `\day`, …, and
//! pdfTeX's `\pdfcreationdate`) and file dates, in the local zone unless
//! `SOURCE_DATE_EPOCH` says otherwise.
//!
//! The local zone comes from the `TZif` file of `$TZ` or `/etc/localtime`
//! (RFC 8536), with the POSIX rule of its footer for times after its last
//! transition. Without one, local time is UTC.

use partex_core::DateTime;

/// A time zone: UTC offsets (seconds east) by instant.
#[derive(Clone, Debug, Default)]
pub struct Zone {
    /// Transitions: from this instant on, this offset.
    transitions: Vec<(i64, i32)>,
    /// The offset before the first transition.
    initial: i32,
    /// The rule after the last transition.
    rule: Option<Rule>,
}

/// A POSIX TZ rule: standard time, and daylight saving time between two
/// dates of each year.
#[derive(Clone, Copy, Debug)]
struct Rule {
    std: i32,
    dst: Option<(i32, When, When)>,
}

/// `Mm.w.d/time`: day `d` (0 is Sunday) of week `w` (5 is the last) of
/// month `m`, at `time` seconds of local time.
#[derive(Clone, Copy, Debug)]
struct When {
    month: i64,
    week: i64,
    day: i64,
    time: i64,
}

impl Zone {
    /// The zone of `$TZ`, else `/etc/localtime`, else UTC.
    pub fn local() -> Self {
        let path = match std::env::var("TZ") {
            Ok(tz) if !tz.is_empty() => {
                let tz = tz.strip_prefix(':').unwrap_or(&tz).to_owned();
                if tz.starts_with('/') {
                    tz
                } else if let Some(rule) = parse_rule(tz.as_bytes()) {
                    // a rule, not a zone name
                    if !std::path::Path::new("/usr/share/zoneinfo")
                        .join(&tz)
                        .exists()
                    {
                        return Self {
                            rule: Some(rule),
                            ..Self::default()
                        };
                    }
                    format!("/usr/share/zoneinfo/{tz}")
                } else {
                    format!("/usr/share/zoneinfo/{tz}")
                }
            }
            _ => String::from("/etc/localtime"),
        };
        std::fs::read(path)
            .ok()
            .and_then(|d| parse_tzif(&d))
            .unwrap_or_default()
    }

    /// The UTC offset at instant `t` (seconds since the epoch).
    pub fn offset(&self, t: i64) -> i32 {
        match self.transitions.iter().rposition(|&(at, _)| at <= t) {
            Some(i) if i + 1 < self.transitions.len() || self.rule.is_none() => {
                self.transitions[i].1
            }
            None if !self.transitions.is_empty() => self.initial,
            _ => self.rule.map_or(self.initial, |r| r.offset(t)),
        }
    }
}

impl Rule {
    fn offset(&self, t: i64) -> i32 {
        let Some((dst, start, end)) = self.dst else {
            return self.std;
        };
        let (y, ..) = civil(t + i64::from(self.std));
        // the changes, in UTC: the start is told in standard time, the end
        // in daylight saving time
        let on = start.local(y) - i64::from(self.std);
        let off = end.local(y) - i64::from(dst);
        let in_dst = if on < off {
            on <= t && t < off
        } else {
            !(off <= t && t < on)
        };
        if in_dst { dst } else { self.std }
    }
}

impl When {
    /// The local time of the change in year `y`, as seconds since the
    /// epoch (of local time).
    fn local(&self, y: i64) -> i64 {
        let first = days_from_civil(y, self.month, 1);
        let wd = (first + 4).rem_euclid(7); // 1970-01-01 was a Thursday
        let mut day = 1 + (self.day - wd).rem_euclid(7) + 7 * (self.week - 1);
        let len = days_from_civil(y + i64::from(self.month == 12), self.month % 12 + 1, 1) - first;
        while day > len {
            day -= 7;
        }
        (first + day - 1) * 86_400 + self.time
    }
}

fn be32(d: &[u8], i: usize) -> Option<i64> {
    Some(i64::from(i32::from_be_bytes(
        d.get(i..i + 4)?.try_into().ok()?,
    )))
}

fn be64(d: &[u8], i: usize) -> Option<i64> {
    Some(i64::from_be_bytes(d.get(i..i + 8)?.try_into().ok()?))
}

/// RFC 8536: a `TZif` file.
fn parse_tzif(d: &[u8]) -> Option<Zone> {
    if !d.starts_with(b"TZif") {
        return None;
    }
    let counts = |at: usize| -> Option<[usize; 6]> {
        let mut c = [0; 6];
        for (k, v) in c.iter_mut().enumerate() {
            *v = usize::try_from(be32(d, at + 20 + 4 * k)?).ok()?;
        }
        Some(c)
    };
    let [isut, isstd, leap, time, types, chars] = counts(0)?;
    let v1_len = 44 + time * 5 + types * 6 + chars + leap * 8 + isstd + isut;
    // version 2 and later: a second header and 64-bit times, then the
    // footer
    let (base, wide) = if d[4] >= b'2' { (v1_len, 8) } else { (0, 4) };
    let [isut, isstd, leap, time, types, chars] = if wide == 8 {
        counts(base)?
    } else {
        [isut, isstd, leap, time, types, chars]
    };
    let at = base + 44;
    let idx = at + time * wide;
    let tt = idx + time;
    let offset_of = |k: usize| be32(d, tt + 6 * k).and_then(|o| i32::try_from(o).ok());
    let mut transitions = Vec::with_capacity(time);
    for i in 0..time {
        let t = if wide == 8 {
            be64(d, at + 8 * i)?
        } else {
            be32(d, at + 4 * i)?
        };
        transitions.push((t, offset_of(usize::from(*d.get(idx + i)?))?));
    }
    let end = tt + types * 6 + chars + leap * (wide + 4) + isstd + isut;
    let rule = (wide == 8)
        .then(|| d.get(end..))
        .flatten()
        .and_then(|f| f.strip_prefix(b"\n"))
        .and_then(|f| f.split(|&c| c == b'\n').next())
        .and_then(parse_rule);
    Some(Zone {
        transitions,
        initial: offset_of(0)?,
        rule,
    })
}

/// A POSIX TZ string such as `CET-1CEST,M3.5.0,M10.5.0/3` or `<+03>-3`.
fn parse_rule(s: &[u8]) -> Option<Rule> {
    let mut p = Posix { s, i: 0 };
    p.name()?;
    let std = -p.offset()?;
    if p.i == s.len() {
        return Some(Rule { std, dst: None });
    }
    p.name()?;
    let dst = if matches!(p.peek(), Some(b'+' | b'-' | b'0'..=b'9')) {
        -p.offset()?
    } else {
        std + 3600
    };
    // (the default rule when none is given is the US one)
    let (start, end) = if p.eat(b',') {
        let start = p.when()?;
        p.eat(b',').then_some(())?;
        (start, p.when()?)
    } else {
        (
            When {
                month: 3,
                week: 2,
                day: 0,
                time: 7200,
            },
            When {
                month: 11,
                week: 1,
                day: 0,
                time: 7200,
            },
        )
    };
    Some(Rule {
        std,
        dst: Some((dst, start, end)),
    })
}

struct Posix<'a> {
    s: &'a [u8],
    i: usize,
}

impl Posix<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        let ok = self.peek() == Some(c);
        self.i += usize::from(ok);
        ok
    }

    fn name(&mut self) -> Option<()> {
        if self.eat(b'<') {
            while !self.eat(b'>') {
                self.peek()?;
                self.i += 1;
            }
            return Some(());
        }
        let from = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        (self.i - from >= 3).then_some(())
    }

    fn num(&mut self) -> Option<i64> {
        let from = self.i;
        let mut n = 0i64;
        while let Some(c @ b'0'..=b'9') = self.peek() {
            n = n * 10 + i64::from(c - b'0');
            self.i += 1;
        }
        (self.i > from).then_some(n)
    }

    /// `[+-]hh[:mm[:ss]]`, in seconds.
    fn time(&mut self) -> Option<i64> {
        let sign = if self.eat(b'-') {
            -1
        } else {
            self.eat(b'+');
            1
        };
        let mut t = self.num()? * 3600;
        if self.eat(b':') {
            t += self.num()? * 60;
            if self.eat(b':') {
                t += self.num()?;
            }
        }
        Some(sign * t)
    }

    fn offset(&mut self) -> Option<i32> {
        i32::try_from(self.time()?).ok()
    }

    fn when(&mut self) -> Option<When> {
        // (only the `M` form: the Julian-day forms are not used by tzdata)
        self.eat(b'M').then_some(())?;
        let month = self.num()?;
        self.eat(b'.').then_some(())?;
        let week = self.num()?;
        self.eat(b'.').then_some(())?;
        let day = self.num()?;
        let time = if self.eat(b'/') { self.time()? } else { 7200 };
        Some(When {
            month,
            week,
            day,
            time,
        })
    }
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date and time of `t` seconds since the epoch: year, month,
/// day, hour, minute, second.
fn civil(t: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, secs / 3600, secs / 60 % 60, secs % 60)
}

/// The job's clock: when it started, and how web2c tells times.
#[derive(Clone, Debug)]
pub struct Clock {
    zone: Zone,
    /// The start: `SOURCE_DATE_EPOCH` if set, else the clock.
    start: i64,
    /// The clock when the job started.
    now: i64,
    /// `SOURCE_DATE_EPOCH` is set.
    epoch_set: bool,
    /// ... and `FORCE_SOURCE_DATE=1` makes TeX's own dates use it too.
    forced: bool,
}

impl Clock {
    /// From the environment, as web2c reads it. Knuth's TeX is built
    /// `onlyTeX` and reads neither variable.
    pub fn from_env(only_tex: bool) -> Self {
        let var = |n: &str| std::env::var(n).ok().filter(|_| !only_tex);
        let epoch = var("SOURCE_DATE_EPOCH").and_then(|s| s.trim().parse::<i64>().ok());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        Self {
            zone: Zone::local(),
            start: epoch.unwrap_or(now),
            now,
            epoch_set: epoch.is_some(),
            forced: var("FORCE_SOURCE_DATE").is_some_and(|v| v == "1"),
        }
    }

    /// texmfmp.c's `get_date_and_time`: `\time`, `\day`, `\month`,
    /// `\year` (the clock in local time, or `SOURCE_DATE_EPOCH` in UTC
    /// when forced).
    pub fn start(&self) -> DateTime {
        let (t, utc) = if self.forced {
            (self.start, true)
        } else {
            (self.now, false)
        };
        let (y, m, d, hh, mm, _) = civil(self.local(t, utc));
        DateTime {
            year: i32::try_from(y).unwrap_or(1970),
            month: i32::try_from(m).unwrap_or(1),
            day: i32::try_from(d).unwrap_or(1),
            minutes: i32::try_from(hh * 60 + mm).unwrap_or(0),
        }
    }

    fn local(&self, t: i64, utc: bool) -> i64 {
        if utc {
            t
        } else {
            t + i64::from(self.zone.offset(t))
        }
    }

    /// texmfmp.c's `makepdftime`: `D:YYYYmmddHHMMSS` and the zone (`Z`,
    /// or `+HH'MM'`).
    fn pdf_time(&self, t: i64, utc: bool) -> Vec<u8> {
        use std::fmt::Write;
        let off = if utc { 0 } else { self.zone.offset(t) };
        let (year, month, day, hour, minute, second) = civil(t + i64::from(off));
        let mut s = format!(
            "D:{year:04}{month:02}{day:02}{hour:02}{minute:02}{:02}",
            second.min(59)
        );
        if off == 0 {
            s.push('Z');
        } else {
            // (C's `/` truncates, and the minutes are printed unsigned)
            let (hours, minutes) = (off / 3600, (off / 60 % 60).abs());
            let _ = write!(s, "{hours:+03}'{minutes:02}'");
        }
        s.into_bytes()
    }

    /// pdfTeX's `\pdfcreationdate`: the start, in UTC if
    /// `SOURCE_DATE_EPOCH` is set.
    pub fn creation_date(&self) -> Vec<u8> {
        self.pdf_time(self.start, self.epoch_set)
    }

    /// pdfTeX's `\pdffilemoddate` of a file changed at `t`: in UTC if
    /// `SOURCE_DATE_EPOCH` is set and forced.
    pub fn file_date(&self, t: i64) -> Vec<u8> {
        self.pdf_time(t, self.epoch_set && self.forced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for days in [-1_000_000, -1, 0, 59, 60, 10_957, 20_000, 1_000_000] {
            let (y, m, d, ..) = civil(days * 86_400);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil(1_700_000_000), (2023, 11, 14, 22, 13, 20));
    }

    #[test]
    fn posix_rules() {
        let cet = parse_rule(b"CET-1CEST,M3.5.0,M10.5.0/3").expect("rule");
        // 2024: CEST from 2024-03-31 01:00 UTC to 2024-10-27 01:00 UTC
        assert_eq!(cet.offset(1_711_846_799), 3600);
        assert_eq!(cet.offset(1_711_846_800), 7200);
        assert_eq!(cet.offset(1_729_990_799), 7200);
        assert_eq!(cet.offset(1_729_990_800), 3600);
        let riyadh = parse_rule(b"<+03>-3").expect("rule");
        assert_eq!(riyadh.offset(0), 10_800);
        let india = parse_rule(b"IST-5:30").expect("rule");
        assert_eq!(india.offset(0), 19_800);
    }

    #[test]
    fn pdf_dates() {
        let c = Clock {
            zone: Zone {
                rule: parse_rule(b"<+03>-3"),
                ..Zone::default()
            },
            start: 1_700_000_000,
            now: 1_700_000_000,
            epoch_set: false,
            forced: false,
        };
        assert_eq!(c.creation_date(), b"D:20231115011320+03'00'");
        let india = Clock {
            zone: Zone {
                rule: parse_rule(b"<-0330>3:30"),
                ..Zone::default()
            },
            ..c
        };
        assert_eq!(india.file_date(1_700_000_000), b"D:20231114184320-03'30'");
    }
}
