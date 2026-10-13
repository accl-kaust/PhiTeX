//! Biber::Date::Format (DateTime::Format::Builder parsers for ISO 8601-1
//! and the ISO 8601-2 extensions biber accepts) and Utils'
//! `parse_date_range` / `parse_date_unspecified`.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::unicode;

/// A parsed date (a DateTime plus the parser's flags).
#[derive(Clone, Debug, Default)]
pub struct Date {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub tz: Option<String>,
    pub missing_month: bool,
    pub missing_day: bool,
    pub missing_time: bool,
    pub approximate: bool,
    pub uncertain: bool,
    pub yeardivision: Option<&'static str>,
    pub atos: BTreeMap<String, String>,
}

impl Date {
    pub fn resolve(&self, v: i64) -> String {
        let s = alloc::format!("{v}");
        self.atos.get(&s).cloned().unwrap_or(s)
    }
    pub fn era(&self) -> &'static str {
        if self.year <= 0 { "bce" } else { "ce" }
    }
}

/// Result of parsing one side: `None` = parse failure (undef), `Some(None)`
/// = empty or open (0), `Some(Some(d))` = a date.
pub type Side = Option<Option<Date>>;

pub fn yeardivision(n: u32) -> Option<&'static str> {
    Some(match n {
        21 => "spring",
        22 => "summer",
        23 => "autumn",
        24 => "winter",
        25 => "springN",
        26 => "summerN",
        27 => "autumnN",
        28 => "winterN",
        29 => "springS",
        30 => "summerS",
        31 => "autumnS",
        32 => "winterS",
        33 => "Q1",
        34 => "Q2",
        35 => "Q3",
        36 => "Q4",
        37 => "QD1",
        38 => "QD2",
        39 => "QD3",
        40 => "S1",
        41 => "S2",
        _ => return None,
    })
}

fn leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn month_days(y: i64, m: u32) -> u32 {
    match m {
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap(y) {
                29
            } else {
                28
            }
        }
        _ => 31,
    }
}

fn digits(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit())
}

/// `parse_date`: `None` for a bad parse.
pub fn parse_date(s: Option<&str>) -> Side {
    let s = match s {
        Some(s) if crate::model::truthy(s) => s,
        _ => return Some(None),
    };
    if s == ".." {
        return Some(None);
    }
    let mut d = Date::default();
    // script mapping of non-ASCII digit runs
    let mut input = String::new();
    {
        let cs: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < cs.len() {
            if unicode::is_digit(cs[i]) {
                let st = i;
                while i < cs.len() && unicode::is_digit(cs[i]) {
                    i += 1;
                }
                let num: String = cs[st..i].iter().collect();
                let ascii: String = cs[st..i]
                    .iter()
                    .map(|&c| {
                        char::from_digit(unicode::digit_value(c).unwrap_or(0), 10).unwrap_or('0')
                    })
                    .collect();
                if num != ascii {
                    let r = unicode::num(&num)
                        .map(|v| alloc::format!("{v}"))
                        .unwrap_or_default();
                    d.atos.insert(ascii.clone(), num.clone());
                    d.atos.insert(r, num.clone());
                }
                input.push_str(&ascii);
            } else {
                input.push(cs[i]);
                i += 1;
            }
        }
    }
    let strip_flag = |s: &str, f: char| -> Option<String> {
        // ^\s*(.+?)\s*F\s*$
        let t = s.trim_end();
        let t = t.strip_suffix(f)?;
        let t = t.trim_end().trim_start();
        if t.is_empty() {
            return None;
        }
        Some(t.into())
    };
    if let Some(x) = strip_flag(&input, '?') {
        input = x;
        d.uncertain = true;
    }
    if let Some(x) = strip_flag(&input, '~') {
        input = x;
        d.approximate = true;
    }
    if let Some(x) = strip_flag(&input, '%') {
        input = x;
        d.uncertain = true;
        d.approximate = true;
    }
    if let Some(x) = input.strip_suffix('Z') {
        input = x.into();
        d.tz = Some("UTC".into());
    } else if input.len() >= 6 {
        let tail = &input[input.len() - 6..];
        let b = tail.as_bytes();
        if (b[0] == b'+' || b[0] == b'-')
            && b[1].is_ascii_digit()
            && b[2].is_ascii_digit()
            && b[3] == b':'
            && b[4].is_ascii_digit()
            && b[5].is_ascii_digit()
        {
            d.tz = Some(alloc::format!("{}{}", &tail[..3], &tail[4..]));
            input.truncate(input.len() - 6);
        }
    }
    // (-?\d{4})-([23]\d|4[01])
    {
        let (neg, body) = match input.strip_prefix('-') {
            Some(b) => (true, b),
            None => (false, input.as_str()),
        };
        if body.len() == 7 && digits(&body[..4], 4) && &body[4..5] == "-" && digits(&body[5..], 2) {
            let n: u32 = body[5..].parse().unwrap_or(0);
            if (20..=41).contains(&n) {
                d.yeardivision = yeardivision(n);
                let y = alloc::format!("{}{}", if neg { "-" } else { "" }, &body[..4]);
                input = y;
            }
        }
    }
    let len = input.chars().count();
    let ystr = |s: &str| -> Option<(i64, usize)> {
        let (neg, b) = match s.strip_prefix('-') {
            Some(b) => (true, b),
            None => (false, s),
        };
        if b.len() < 4 || !digits(&b[..4], 4) {
            return None;
        }
        let y: i64 = b[..4].parse().ok()?;
        Some((if neg { -y } else { y }, 4 + usize::from(neg)))
    };
    let mut ok = false;
    if [19, 20, 23, 24].contains(&len) {
        if let Some((y, o)) = ystr(&input) {
            let r = &input[o..];
            let b = r.as_bytes();
            if b.len() >= 15
                && b[0] == b'-'
                && digits(&r[1..3], 2)
                && b[3] == b'-'
                && digits(&r[4..6], 2)
                && b[6] == b'T'
                && digits(&r[7..9], 2)
                && b[9] == b':'
                && digits(&r[10..12], 2)
                && b[12] == b':'
                && digits(&r[13..15], 2)
                && (b.len() == 15 || (b.len() == 19 && b[15] == b'.' && digits(&r[16..19], 3)))
            {
                d.year = y;
                d.month = r[1..3].parse().unwrap_or(0);
                d.day = r[4..6].parse().unwrap_or(0);
                d.hour = r[7..9].parse().unwrap_or(0);
                d.minute = r[10..12].parse().unwrap_or(0);
                d.second = r[13..15].parse().unwrap_or(0);
                ok = true;
            }
        }
    } else if [10, 11].contains(&len) {
        if let Some((y, o)) = ystr(&input) {
            let r = &input[o..];
            let b = r.as_bytes();
            if b.len() == 6
                && b[0] == b'-'
                && digits(&r[1..3], 2)
                && b[3] == b'-'
                && digits(&r[4..6], 2)
            {
                d.year = y;
                d.month = r[1..3].parse().unwrap_or(0);
                d.day = r[4..6].parse().unwrap_or(0);
                d.missing_time = true;
                ok = true;
            }
        }
    } else if [7, 8].contains(&len) {
        if let Some((y, o)) = ystr(&input) {
            let r = &input[o..];
            let b = r.as_bytes();
            if b.len() == 3 && b[0] == b'-' && digits(&r[1..3], 2) {
                d.year = y;
                d.month = r[1..3].parse().unwrap_or(0);
                d.day = 1;
                d.missing_day = true;
                d.missing_time = true;
                ok = true;
            }
        }
    } else if [4, 5].contains(&len) {
        if let Some((y, o)) = ystr(&input) {
            if o == input.len() {
                d.year = y;
                d.month = 1;
                d.day = 1;
                d.missing_month = true;
                d.missing_day = true;
                d.missing_time = true;
                ok = true;
            }
        }
    }
    if !ok {
        if let Some(r) = input.strip_prefix('Y') {
            let (neg, b) = match r.strip_prefix('-') {
                Some(b) => (true, b),
                None => (false, r),
            };
            if b.len() >= 5 && b.bytes().all(|c| c.is_ascii_digit()) {
                let y: i64 = b.parse().ok()?;
                d.year = if neg { -y } else { y };
                d.month = 1;
                d.day = 1;
                d.missing_month = true;
                d.missing_day = true;
                d.missing_time = true;
                ok = true;
            }
        }
    }
    if !ok {
        return None;
    }
    // DateTime->new validation
    if !(1..=12).contains(&d.month)
        || d.day < 1
        || d.day > month_days(d.year, d.month)
        || d.hour > 23
        || d.minute > 59
        || d.second > 59
    {
        return None;
    }
    Some(Some(d))
}

/// `parse_date_unspecified`.
fn unspecified(d: &str) -> Option<(String, String, &'static str)> {
    let b = d.as_bytes();
    let dash = |i: usize| b.get(i) == Some(&b'-');
    if b.len() == 4 && digits(&d[..3], 3) && b[3] == b'X' {
        return Some((
            alloc::format!("{}0", &d[..3]),
            alloc::format!("{}9", &d[..3]),
            "yearindecade",
        ));
    }
    if b.len() == 4 && digits(&d[..2], 2) && &d[2..] == "XX" {
        return Some((
            alloc::format!("{}00", &d[..2]),
            alloc::format!("{}99", &d[..2]),
            "yearincentury",
        ));
    }
    if b.len() == 7 && digits(&d[..4], 4) && dash(4) && &d[5..] == "XX" {
        return Some((
            alloc::format!("{}-01", &d[..4]),
            alloc::format!("{}-12", &d[..4]),
            "monthinyear",
        ));
    }
    if b.len() == 10
        && digits(&d[..4], 4)
        && dash(4)
        && digits(&d[5..7], 2)
        && dash(7)
        && &d[8..] == "XX"
    {
        let y: i64 = d[..4].parse().ok()?;
        let m: u32 = d[5..7].parse().ok()?;
        let md = match &d[5..7] {
            "09" | "04" | "06" | "11" => 30,
            "02" => {
                if leap(y) {
                    29
                } else {
                    28
                }
            }
            _ => 31,
        };
        let _ = m;
        return Some((
            alloc::format!("{}-01", &d[..7]),
            alloc::format!("{}-{md}", &d[..7]),
            "dayinmonth",
        ));
    }
    if b.len() == 10
        && digits(&d[..4], 4)
        && dash(4)
        && &d[5..7] == "XX"
        && dash(7)
        && &d[8..] == "XX"
    {
        return Some((
            alloc::format!("{}-01-01", &d[..4]),
            alloc::format!("{}-12-31", &d[..4]),
            "dayinyear",
        ));
    }
    None
}

/// What `parse_date_range` found.
pub struct Range {
    pub start: Side,
    pub end: Side,
    pub sep: bool,
    pub unspec: Option<&'static str>,
    pub dateunknown: bool,
    pub enddateunknown: bool,
    /// The whole pattern failed (`unless ($sd or $ed)`).
    pub nothing: bool,
}

pub fn parse_date_range(s: &str) -> Range {
    let mut r = Range {
        start: None,
        end: None,
        sep: false,
        unspec: None,
        dateunknown: false,
        enddateunknown: false,
        nothing: false,
    };
    let slashes = s.matches('/').count();
    if slashes > 1 {
        r.nothing = true;
        return r;
    }
    let (mut sd, mut ed): (Option<String>, Option<String>) = match s.find('/') {
        Some(i) => {
            r.sep = true;
            (
                Some(&s[..i]).filter(|x| !x.is_empty()).map(Into::into),
                Some(&s[i + 1..]).filter(|x| !x.is_empty()).map(Into::into),
            )
        }
        None => (Some(s).filter(|x| !x.is_empty()).map(Into::into), None),
    };
    let t = |x: &Option<String>| x.as_deref().is_some_and(crate::model::truthy);
    if !t(&sd) && !t(&ed) {
        r.nothing = true;
        return r;
    }
    if sd.as_deref().is_some_and(|x| x.contains('X')) {
        match unspecified(sd.as_deref().unwrap_or("")) {
            Some((a, b, u)) => {
                sd = Some(a);
                ed = Some(b);
                r.sep = true;
                r.unspec = Some(u);
            }
            None => {
                sd = None;
                ed = None;
                r.sep = false;
            }
        }
    }
    if r.sep && !t(&sd) {
        r.dateunknown = true;
    }
    if r.sep && !t(&ed) {
        r.enddateunknown = true;
    }
    r.start = parse_date(sd.as_deref());
    r.end = parse_date(ed.as_deref());
    r
}

/// Utils `tzformat`.
pub fn tzformat(tz: &str) -> String {
    let b = tz.as_bytes();
    if b.len() >= 3
        && (b[0] == b'+' || b[0] == b'-')
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
    {
        let mut rest = &tz[3..];
        if let Some(x) = rest.strip_prefix(':') {
            rest = x;
        }
        let mm = if rest.len() >= 2 && digits(&rest[..2], 2) {
            &rest[..2]
        } else {
            ""
        };
        return alloc::format!(
            "{}{}",
            &tz[..3],
            if crate::model::truthy(mm) {
                alloc::format!("\\bibtzminsep {mm}")
            } else {
                String::new()
            }
        );
    }
    if tz == "UTC" {
        return "Z".into();
    }
    tz.into()
}
