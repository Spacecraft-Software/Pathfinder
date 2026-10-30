//! Pathfinder: jq 1.8.1's time builtins as its release binary runs them,
//! on glibc 2.39 (M23).
//!
//! jq's `gmtime`, `localtime`, `mktime`, `strftime`, `strflocaltime` and
//! `strptime` are thin wrappers over the C library (`src/builtin.c`), so
//! what they answer is glibc's: `timegm` normalising out-of-range fields
//! and returning -1 as its error, years far past 9999, and the C locale's
//! `strftime` and `strptime` conversions, flags and widths. jq's release
//! binary links glibc 2.39 statically (Ubuntu 24.04), and this transcribes
//! that release's `time/strftime_l.c` (`__strftime_internal`),
//! `time/strptime_l.c` (`__strptime_internal`) and `time/mktime.c`
//! (`__mktime_internal`), keeping their names, their order of checks and
//! the reads past their tables that jq's binary makes. Only the system
//! time zone's rules come from `jiff`.
//!
//! The locale is C: jq calls `setlocale(LC_ALL, "")`, and a static glibc
//! finds no other locale on a host without glibc's locale files. Past
//! about year 5 881 600, glibc's daylight saving rule overflows an `int`
//! (the day count in `compute_change`); this keeps applying the rule.
//! Where `localtime` fails inside `strflocaltime`, jq aborts on an
//! assertion; this raises `localtime`'s error.
//!
//! The file's licence follows from that: glibc's text is
//! LGPL-2.1-or-later, jq's `set_tm_wday`/`set_tm_yday` are MIT, and
//! Pathfinder's own is GPL-3.0-or-later or MIT (`REUSE.toml`).

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::num::Wrapping;

/// `struct tm`, with glibc's `tm_gmtoff` and `tm_zone`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tm {
    pub year: i32,
    pub mon: i32,
    pub mday: i32,
    pub hour: i32,
    pub min: i32,
    pub sec: i32,
    pub wday: i32,
    pub yday: i32,
    pub isdst: i32,
    pub gmtoff: i64,
    /// `None` for C's `NULL`.
    pub zone: Option<String>,
}

const MON_YDAY: [[i32; 13]; 2] = [
    [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334, 365],
    [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335, 366],
];

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (month 1..=12).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day count since 1970-01-01: year, month 1..=12, day.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `gmtime_r`: `None` where the year does not fit an `int` (`EOVERFLOW`).
pub fn gmtime(t: i64) -> Option<Tm> {
    let days = t.div_euclid(86_400);
    let rem = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let year = i32::try_from(y - 1900).ok()?;
    let yday = days - days_from_civil(y, 1, 1);
    Some(Tm {
        year,
        mon: (m - 1) as i32,
        mday: d as i32,
        hour: (rem / 3600) as i32,
        min: (rem % 3600 / 60) as i32,
        sec: (rem % 60) as i32,
        wday: (days + 4).rem_euclid(7) as i32,
        yday: yday as i32,
        isdst: 0,
        gmtoff: 0,
        zone: Some("GMT".into()),
    })
}

/// glibc's `leapyear`, of a `tm_year`.
fn leapyear(year: i64) -> usize {
    usize::from(is_leap(year + 1900))
}

/// `ydhms_diff`: (year1-yday1 hour1:min1:sec1) - (year0-yday0
/// hour0:min0:sec0) in seconds, the years counted from 1900.
#[expect(clippy::too_many_arguments, reason = "glibc's signature")]
fn ydhms_diff(
    year1: i64,
    yday1: i64,
    hour1: i32,
    min1: i32,
    sec1: i32,
    year0: i64,
    yday0: i64,
    hour0: i32,
    min0: i32,
    sec0: i64,
) -> Option<i64> {
    let a4 = (year1 >> 2) + (1900 >> 2) - i64::from(year1.trailing_zeros() >= 2);
    let b4 = (year0 >> 2) + (1900 >> 2) - i64::from(year0.trailing_zeros() >= 2);
    let a100 = (a4 + i64::from(a4 < 0)) / 25 - i64::from(a4 < 0);
    let b100 = (b4 + i64::from(b4 < 0)) / 25 - i64::from(b4 < 0);
    let (a400, b400) = (a100 >> 2, b100 >> 2);
    let leap_days = (a4 - b4) - (a100 - b100) + (a400 - b400);
    let days = (year1 - year0)
        .checked_mul(365)?
        .checked_add(yday1 - yday0 + leap_days)?;
    // In `long_int`, as C's left-to-right `24 * days + hour1 - hour0`.
    let h = days.checked_mul(24)? + i64::from(hour1) - i64::from(hour0);
    let m = h.checked_mul(60)? + i64::from(min1) - i64::from(min0);
    m.checked_mul(60)?
        .checked_add(i64::from(sec1))?
        .checked_sub(sec0)
}

fn tm_diff(year: i64, yday: i64, hour: i32, min: i32, sec: i32, tm: &Tm) -> Option<i64> {
    ydhms_diff(
        year,
        yday,
        hour,
        min,
        sec,
        i64::from(tm.year),
        i64::from(tm.yday),
        tm.hour,
        tm.min,
        i64::from(tm.sec),
    )
}

/// `ranged_convert`: `convert` of `t`, or of the nearest time it can
/// convert, `t` moved there.
fn ranged_convert(convert: fn(i64) -> Option<Tm>, t: &mut i64) -> Option<Tm> {
    if let Some(tm) = convert(*t) {
        return Some(tm);
    }
    let (mut bad, mut ok) = (*t, 0i64);
    let mut oktm = None;
    loop {
        // `long_int_avg`, rounding up.
        let mid = (ok >> 1) + (bad >> 1) + ((ok | bad) & 1);
        if mid == ok || mid == bad {
            break;
        }
        if let Some(tm) = convert(mid) {
            ok = mid;
            oktm = Some(tm);
        } else {
            bad = mid;
        }
    }
    let tm = oktm?;
    *t = ok;
    Some(tm)
}

/// Whether two `tm_isdst` differ (`isdst_differ`).
fn isdst_differ(a: i32, b: i32) -> bool {
    (a == 0) != (b == 0) && a >= 0 && b >= 0
}

/// `__mktime_internal`: the time `convert` turns into `tm`, `tm`
/// normalised to it, `offset` the guess carried between calls; -1 when
/// there is none, `tm` then left as it was.
#[expect(
    clippy::too_many_lines,
    reason = "glibc's one function, kept whole to stay comparable"
)]
fn mktime_internal(tm: &mut Tm, convert: fn(i64) -> Option<Tm>, offset: &mut i64) -> i64 {
    let mut remaining_probes = 6;
    let (min, hour, mday, mon) = (tm.min, tm.hour, tm.mday, tm.mon);
    let isdst = tm.isdst;
    let mut dst2 = false;
    let mon_remainder = mon % 12;
    let negative = i32::from(mon_remainder < 0);
    let mon_years = mon / 12 - negative;
    let year = i64::from(tm.year) + i64::from(mon_years);
    let index = usize::try_from(mon_remainder + 12 * negative).unwrap_or(0);
    let mon_yday = i64::from(MON_YDAY[leapyear(year)][index]) - 1;
    let yday = mon_yday + i64::from(mday);
    let sec_requested = tm.sec;
    // `LEAP_SECONDS_POSSIBLE`: seconds out of range are added at the end.
    let sec = sec_requested.clamp(0, 59);
    let negative_offset_guess = offset.wrapping_neg();
    let Some(t0) = ydhms_diff(
        year,
        yday,
        hour,
        min,
        sec,
        70,
        0,
        0,
        0,
        negative_offset_guess,
    ) else {
        return -1;
    };
    let (mut t, mut t1, mut t2) = (t0, t0, t0);
    let mut in_gap = false;
    let mut out = loop {
        let Some(got) = ranged_convert(convert, &mut t) else {
            return -1;
        };
        let Some(dt) = tm_diff(year, yday, hour, min, sec, &got) else {
            return -1;
        };
        if dt == 0 {
            break got;
        }
        if t == t1
            && t != t2
            && (got.isdst < 0
                || if isdst < 0 {
                    dst2 <= (got.isdst != 0)
                } else {
                    (isdst != 0) != (got.isdst != 0)
                })
        {
            // Oscillating over a spring-forward gap: a time `dt` away.
            in_gap = true;
            break got;
        }
        remaining_probes -= 1;
        if remaining_probes == 0 {
            return -1;
        }
        (t1, t2) = (t2, t);
        let Some(next) = t.checked_add(dt) else {
            return -1;
        };
        t = next;
        dst2 = got.isdst != 0;
    };
    if !in_gap && isdst_differ(isdst, out.isdst) {
        // The wrong `tm_isdst`: use the offset of a neighbouring time
        // with the right one, or else assume a one-hour difference.
        let dst_difference = i64::from(isdst == 0) - i64::from(out.isdst == 0);
        let stride: i64 = 601_200;
        let delta_bound: i64 = 457_243_209 / 2 + stride;
        let mut delta = stride;
        let mut found = false;
        'probe: while delta < delta_bound {
            for direction in [-1, 1] {
                let Some(mut ot) = t.checked_add(delta * direction) else {
                    continue;
                };
                let Some(otm) = ranged_convert(convert, &mut ot) else {
                    return -1;
                };
                if isdst_differ(isdst, otm.isdst) {
                    continue;
                }
                let gt = tm_diff(year, yday, hour, min, sec, &otm).and_then(|d| ot.checked_add(d));
                if let Some(n) = gt.and_then(convert) {
                    t = gt.unwrap_or(t);
                    out = n;
                    found = true;
                    break 'probe;
                }
            }
            delta += stride;
        }
        if !found {
            t += 3600 * dst_difference;
            match convert(t) {
                Some(n) => out = n,
                None => return -1,
            }
        }
    }
    *offset = t.wrapping_sub(t0).wrapping_sub(negative_offset_guess);
    if sec_requested != out.sec {
        let adjustment =
            i64::from(sec == 0 && out.sec == 60) - i64::from(sec) + i64::from(sec_requested);
        let Some(adjusted) = t.checked_add(adjustment) else {
            return -1;
        };
        t = adjusted;
        match convert(t) {
            Some(n) => out = n,
            None => return -1,
        }
    }
    *tm = out;
    t
}

/// `timegm`: the seconds of `tm` in UTC, `tm` normalised; -1 where there
/// are none, `tm` then left as it was. -1 is also the answer for one
/// second before the epoch, which jq takes as the error.
pub fn timegm(tm: &mut Tm) -> i64 {
    let mut offset = 0;
    mktime_internal(tm, gmtime, &mut offset)
}

/// The system time zone's offset, daylight saving flag and abbreviation
/// at `t`, as `localtime_r` finds them (`TZ`, else `/etc/localtime`).
fn local_info(t: i64) -> (i64, bool, String) {
    use jiff::{tz::TimeZone, Timestamp};
    let tz = TimeZone::system();
    // Past `jiff`'s range a zone's rules repeat every 400 years, the
    // Gregorian cycle, as the POSIX rule closing a zone file does; before
    // it, the earliest rule stands.
    const CYCLE: i64 = 12_622_780_800;
    let max = Timestamp::MAX.as_second();
    let t = if t > max {
        max - (max - t).rem_euclid(CYCLE)
    } else {
        t
    };
    let t = t.max(Timestamp::MIN.as_second());
    let ts = Timestamp::from_second(t).unwrap_or(Timestamp::UNIX_EPOCH);
    let info = tz.to_offset_info(ts);
    (
        i64::from(info.offset().seconds()),
        info.dst().is_dst(),
        info.abbreviation().into(),
    )
}

/// `localtime_r`.
pub fn localtime(t: i64) -> Option<Tm> {
    let (off, dst, abbr) = local_info(t);
    let mut tm = gmtime(t.checked_add(off)?)?;
    tm.isdst = i32::from(dst);
    tm.gmtoff = off;
    tm.zone = Some(abbr);
    Some(tm)
}

/// `mktime`, `tm.isdst` the daylight saving time asked for (-1: either):
/// the instant whose local time `tm` is, `tm` normalised to it; -1 when
/// there is none. glibc keeps its guess of the offset between calls
/// (`localtime_offset`), and the guess decides between the two readings
/// of a time the clocks go back over; so this keeps one per thread.
pub fn mktime(tm: &mut Tm) -> i64 {
    std::thread_local! {
        static OFFSET: core::cell::Cell<i64> = const { core::cell::Cell::new(0) };
    }
    OFFSET.with(|o| {
        let mut offset = o.get();
        let t = mktime_internal(tm, localtime, &mut offset);
        o.set(offset);
        t
    })
}

/// `jv2tm`: a broken-down time array as a `tm`, each number cut to an
/// `int`; `None` for anything else than numbers. Missing elements are 0.
pub fn from_array(nums: &[Option<f64>]) -> Option<Tm> {
    let mut f = [0i32; 8];
    for (i, n) in nums.iter().take(8).enumerate() {
        let mut d = (*n)?;
        if d.is_nan() {
            return None;
        }
        if i == 0 {
            d -= 1900.0;
        }
        f[i] = if d < f64::from(i32::MIN) {
            i32::MIN
        } else if d > f64::from(i32::MAX) {
            i32::MAX
        } else {
            d as i32
        };
    }
    Some(Tm {
        year: f[0],
        mon: f[1],
        mday: f[2],
        hour: f[3],
        min: f[4],
        sec: f[5],
        wday: f[6],
        yday: f[7],
        ..Tm::default()
    })
}

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A day or month name, `?` out of range (`a_wkday`, `f_month`, …).
fn name(names: &[&'static str], i: i32, abbr: bool) -> &'static str {
    match usize::try_from(i).ok().and_then(|i| names.get(i)) {
        Some(n) if abbr => &n[..3],
        Some(n) => n,
        None => "?",
    }
}

/// `tzname[isdst]` after `tzset`, for a time with no zone of its own: the
/// system zone's abbreviation for standard (or daylight saving) time,
/// looked for over the year to come.
fn tzname(isdst: i32) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    let mut first = None;
    for quarter in 0..4 {
        let (_, dst, abbr) = local_info(now + quarter * 91 * 86_400);
        if dst == (isdst != 0) {
            return abbr;
        }
        first.get_or_insert(abbr);
    }
    first.unwrap_or_default()
}

/// `iso_week_days`.
fn iso_week_days(yday: i32, wday: i32) -> i32 {
    let (w, yday, wday) = (Wrapping, Wrapping(yday), Wrapping(wday));
    (yday - (yday - wday + w(4) + w(378)) % w(7) + w(4) - w(1)).0
}

fn leap(year: i32) -> i32 {
    i32::from(year % 4 == 0 && (year % 100 != 0 || year % 400 == 0))
}

/// The output of one `strftime` call: `add`'s check against `maxsize`.
struct Out {
    buf: Vec<u8>,
    max: usize,
}

/// `strftime` ran out of room (it returns 0).
struct Full;

impl Out {
    /// `add`: `bytes`, padded to `width` with zeros (`pad` `0`) or spaces.
    fn add(&mut self, bytes: &[u8], width: i32, pad: u8) -> Result<(), Full> {
        let n = bytes.len();
        let delta = usize::try_from(width).unwrap_or(0).saturating_sub(n);
        if n + delta >= self.max - self.buf.len() {
            return Err(Full);
        }
        let fill = if pad == b'0' { b'0' } else { b' ' };
        self.buf.resize(self.buf.len() + delta, fill);
        self.buf.extend_from_slice(bytes);
        Ok(())
    }

    /// `cpy`: `add`, in lower or upper case.
    fn cpy(&mut self, s: &[u8], width: i32, pad: u8, lower: bool, upper: bool) -> Result<(), Full> {
        let s: Vec<u8> = if lower {
            s.to_ascii_lowercase()
        } else if upper {
            s.to_ascii_uppercase()
        } else {
            s.to_vec()
        };
        self.add(&s, width, pad)
    }

    /// `do_number_sign_and_padding`, for the digits (and sign) in `num`.
    fn number(&mut self, num: &[u8], digits: i32, mut width: i32, pad: u8) -> Result<(), Full> {
        let mut num = num;
        if pad != b'-' {
            let padding = usize::try_from(digits)
                .unwrap_or(0)
                .saturating_sub(num.len());
            if padding > 0 {
                if pad == b'_' {
                    if padding >= self.max - self.buf.len() {
                        return Err(Full);
                    }
                    self.buf.resize(self.buf.len() + padding, b' ');
                    width = width
                        .saturating_sub(i32::try_from(padding).unwrap_or(i32::MAX))
                        .max(0);
                } else {
                    if usize::try_from(digits).unwrap_or(0) >= self.max - self.buf.len() {
                        return Err(Full);
                    }
                    if let Some(rest) = num.strip_prefix(b"-") {
                        num = rest;
                        self.buf.push(b'-');
                    }
                    self.buf.resize(self.buf.len() + padding, b'0');
                    width = 0;
                }
            }
        }
        self.add(num, width, pad)
    }
}

/// glibc 2.39's `strftime` (`__strftime_internal`) in the C locale, into
/// a string; `None` where it returns 0, the result not fitting `max`
/// bytes with its terminator.
pub fn strftime(fmt: &[u8], tm: &Tm, max: usize) -> Option<Vec<u8>> {
    let mut out = Out {
        buf: Vec::new(),
        max,
    };
    strftime_into(&mut out, fmt, tm).ok()?;
    Some(out.buf)
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per conversion, as glibc's `__strftime_internal`"
)]
fn strftime_into(out: &mut Out, fmt: &[u8], tm: &Tm) -> Result<(), Full> {
    let hour12 = match tm.hour {
        h if h > 12 => h - 12,
        0 => 12,
        h => h,
    };
    let mut f = 0;
    while f < fmt.len() {
        if fmt[f] != b'%' {
            out.add(&fmt[f..=f], -1, 0)?;
            f += 1;
            continue;
        }
        let start = f;
        let mut pad = 0u8;
        let mut upper = false;
        let mut lower = false;
        let mut change_case = false;
        f += 1;
        while let Some(&c) = fmt.get(f) {
            match c {
                b'_' | b'-' | b'0' => pad = c,
                b'^' => upper = true,
                b'#' => change_case = true,
                _ => break,
            }
            f += 1;
        }
        let mut width: i32 = -1;
        if fmt.get(f).is_some_and(u8::is_ascii_digit) {
            width = 0;
            while let Some(&c) = fmt.get(f).filter(|c| c.is_ascii_digit()) {
                width = width.saturating_mul(10).saturating_add(i32::from(c - b'0'));
                f += 1;
            }
        }
        let modifier = match fmt.get(f) {
            Some(&m @ (b'E' | b'O')) => {
                f += 1;
                m
            }
            _ => 0,
        };
        // The conversion; past the end, the `%…` is copied (`bad_format`).
        let conv = fmt.get(f).copied().unwrap_or(0);
        // `bad_format`: the conversion as written, from the last `%`
        // before its end (the conversion itself, for `%E%`).
        let bad = |out: &mut Out, pad: u8, upper: bool| {
            let end = (f + 1).min(fmt.len());
            let from = fmt[start..end]
                .iter()
                .rposition(|&c| c == b'%')
                .map_or(start, |i| start + i);
            out.cpy(&fmt[from..end], width, pad, false, upper)
        };
        enum Do {
            Number(i32, i32),
            SpacePad(i32, i32),
            Sub(&'static str),
            Text(&'static str),
        }
        let year = tm.year.wrapping_add(1900);
        let what = match conv {
            b'%' if modifier == 0 => {
                out.add(b"%", width, pad)?;
                None
            }
            b'a' | b'A' if modifier == 0 => {
                if change_case {
                    (upper, lower) = (true, false);
                }
                Some(Do::Text(name(&DAYS, tm.wday, conv == b'a')))
            }
            b'b' | b'h' | b'B' => {
                // `%b` takes the `#` flag before it rejects `E`, `%B` after.
                if change_case && (conv != b'B' || modifier != b'E') {
                    (upper, lower) = (true, false);
                }
                if modifier == b'E' {
                    bad(out, pad, upper)?;
                    None
                } else {
                    Some(Do::Text(name(&MONTHS, tm.mon, conv != b'B')))
                }
            }
            b'c' if modifier != b'O' => Some(Do::Sub("%a %b %e %H:%M:%S %Y")),
            b'C' => Some(Do::Number(
                1,
                (year / 100).wrapping_sub(i32::from(year % 100 < 0)),
            )),
            b'x' if modifier != b'O' => Some(Do::Sub("%m/%d/%y")),
            b'D' if modifier == 0 => Some(Do::Sub("%m/%d/%y")),
            b'd' if modifier != b'E' => Some(Do::Number(2, tm.mday)),
            b'e' if modifier != b'E' => Some(Do::SpacePad(2, tm.mday)),
            b'F' if modifier == 0 => Some(Do::Sub("%Y-%m-%d")),
            b'H' if modifier != b'E' => Some(Do::Number(2, tm.hour)),
            b'I' if modifier != b'E' => Some(Do::Number(2, hour12)),
            b'k' if modifier != b'E' => Some(Do::SpacePad(2, tm.hour)),
            b'l' if modifier != b'E' => Some(Do::SpacePad(2, hour12)),
            b'j' if modifier != b'E' => Some(Do::Number(3, tm.yday.wrapping_add(1))),
            b'M' if modifier != b'E' => Some(Do::Number(2, tm.min)),
            b'm' if modifier != b'E' => Some(Do::Number(2, tm.mon.wrapping_add(1))),
            b'n' => {
                out.add(b"\n", width, pad)?;
                None
            }
            b't' => {
                out.add(b"\t", width, pad)?;
                None
            }
            b'P' | b'p' => {
                if conv == b'P' {
                    lower = true;
                }
                if change_case {
                    (upper, lower) = (false, true);
                }
                Some(Do::Text(if tm.hour > 11 { "PM" } else { "AM" }))
            }
            b'R' => Some(Do::Sub("%H:%M")),
            b'r' => Some(Do::Sub("%I:%M:%S %p")),
            b'S' if modifier != b'E' => Some(Do::Number(2, tm.sec)),
            b's' => {
                let mut copy = tm.clone();
                let t = mktime(&mut copy);
                out.number(t.to_string().as_bytes(), 1, width, pad)?;
                None
            }
            b'X' if modifier != b'O' => Some(Do::Sub("%H:%M:%S")),
            b'T' => Some(Do::Sub("%H:%M:%S")),
            b'u' => Some(Do::Number(
                1,
                (tm.wday.wrapping_sub(1).wrapping_add(7) % 7).wrapping_add(1),
            )),
            b'U' if modifier != b'E' => Some(Do::Number(
                2,
                tm.yday.wrapping_sub(tm.wday).wrapping_add(7) / 7,
            )),
            b'V' | b'g' | b'G' if modifier != b'E' => {
                let mut year = year;
                let mut days = iso_week_days(tm.yday, tm.wday);
                if days < 0 {
                    year = year.wrapping_sub(1);
                    days = iso_week_days(tm.yday.wrapping_add(365 + leap(year)), tm.wday);
                } else {
                    let d = iso_week_days(tm.yday.wrapping_sub(365 + leap(year)), tm.wday);
                    if d >= 0 {
                        year = year.wrapping_add(1);
                        days = d;
                    }
                }
                Some(match conv {
                    b'g' => Do::Number(2, (year % 100 + 100) % 100),
                    b'G' => Do::Number(1, year),
                    _ => Do::Number(2, days / 7 + 1),
                })
            }
            b'W' if modifier != b'E' => {
                let monday = tm.wday.wrapping_sub(1).wrapping_add(7) % 7;
                Some(Do::Number(
                    2,
                    tm.yday.wrapping_sub(monday).wrapping_add(7) / 7,
                ))
            }
            b'w' if modifier != b'E' => Some(Do::Number(1, tm.wday)),
            b'Y' if modifier != b'O' => Some(Do::Number(1, year)),
            b'y' => Some(Do::Number(2, (tm.year % 100 + 100) % 100)),
            b'Z' => {
                if change_case {
                    (upper, lower) = (false, true);
                }
                let zone = match &tm.zone {
                    Some(z) if !z.is_empty() => z.clone(),
                    _ if tm.isdst >= 0 => tzname(tm.isdst),
                    _ => String::new(),
                };
                out.cpy(zone.as_bytes(), width, pad, lower, upper)?;
                None
            }
            b'z' if tm.isdst < 0 => None,
            b'z' => {
                let mut diff = tm.gmtoff;
                if diff < 0 {
                    out.add(b"-", width, pad)?;
                    diff = -diff;
                } else {
                    out.add(b"+", width, pad)?;
                }
                let diff = diff / 60;
                #[expect(clippy::cast_possible_truncation, reason = "C's `int diff`")]
                let v = ((diff / 60) * 100 + diff % 60) as i32;
                Some(Do::Number(4, v))
            }
            _ => {
                bad(out, pad, upper)?;
                if conv == 0 {
                    break;
                }
                None
            }
        };
        match what {
            Some(Do::Text(s)) => out.cpy(s.as_bytes(), width, pad, lower, upper)?,
            Some(Do::Sub(sub)) => {
                let mut inner = Out {
                    buf: Vec::new(),
                    max: usize::MAX,
                };
                strftime_into(&mut inner, sub.as_bytes(), tm)?;
                let mut s = inner.buf;
                if upper {
                    s.make_ascii_uppercase();
                }
                out.add(&s, width, pad)?;
            }
            Some(Do::Number(d, v) | Do::SpacePad(d, v)) => {
                if matches!(what, Some(Do::SpacePad(..))) && pad != b'0' && pad != b'-' {
                    pad = b'_';
                }
                let digits = d.max(width);
                // C's `unsigned int u = number_value; u = -u`.
                let mut num = if v < 0 {
                    String::from("-")
                } else {
                    String::new()
                };
                num += &v.unsigned_abs().to_string();
                out.number(num.as_bytes(), digits, width, pad)?;
            }
            None => {}
        }
        f += 1;
    }
    Ok(())
}

/// Whether `strptime` reads the locale's strings, the C locale's, or has
/// not decided (`enum ptime_locale_status`). The locale here is the C
/// locale, whose strings are the same, so only the conversions that have
/// no C-locale meaning decide: `%O` with a number (`loc`), `%EC`, `%Ey`
/// and `%EY` (`raw`), and a failed `%c`, `%x`, `%X` or `%r` (`raw`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Decided {
    Not,
    Loc,
    Raw,
}

/// glibc's `strptime` state (`struct __strptime_state`).
#[derive(Clone)]
struct Parse {
    have_i: bool,
    is_pm: bool,
    century: i32,
    want_century: bool,
    want_era: bool,
    want_xday: bool,
    have_wday: bool,
    have_yday: bool,
    have_mon: bool,
    have_mday: bool,
    have_uweek: bool,
    have_wweek: bool,
    week_no: i32,
    decided: Decided,
}

impl Default for Parse {
    fn default() -> Self {
        Self {
            have_i: false,
            is_pm: false,
            century: -1,
            want_century: false,
            want_era: false,
            want_xday: false,
            have_wday: false,
            have_yday: false,
            have_mon: false,
            have_mday: false,
            have_uweek: false,
            have_wweek: false,
            week_no: 0,
            decided: Decided::Not,
        }
    }
}

/// C's `isspace` in the C locale.
pub fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

/// `__mon_yday[leap][i]` as jq's release binary reads it where glibc's
/// unbounded loops leave the table (measured): a loop over it stops at
/// 12, and before the first entry is the common year's 255 and, in a
/// leap year, the row before's 365 (`"2001x00 Fri" | strptime("%Yx%W
/// %a")` has day -257, 2000 one of -365).
fn mon_yday(leap: usize, i: i32) -> i32 {
    match usize::try_from(i) {
        Ok(i) => MON_YDAY[leap].get(i).copied().unwrap_or(0),
        Err(_) if leap == 1 => 365,
        Err(_) => 255,
    }
}

/// glibc 2.39's `strptime` in the C locale: where the input stopped, or
/// `None` when it does not match.
pub fn strptime(input: &[u8], fmt: &[u8], tm: &mut Tm) -> Option<usize> {
    let mut s = Parse::default();
    let rp = parse(input, 0, fmt, tm, &mut s)?;
    if s.have_i && s.is_pm {
        tm.hour += 12;
    }
    if s.century != -1 {
        if s.want_century {
            tm.year = tm.year % 100 + (s.century - 19) * 100;
        } else {
            tm.year = (s.century - 19) * 100;
        }
    }
    if s.want_era && s.want_century && s.century == -1 && tm.year < 69 {
        // No era found but an `E` modifier seen.
        tm.year += 100;
    }
    let leap = |tm: &Tm| usize::from(is_leap(i64::from(tm.year) + 1900));
    let months = |tm: &mut Tm, have_mon: bool, have_mday: bool| {
        let l = leap(tm);
        let mut t_mon = 0;
        while t_mon < 12 && mon_yday(l, t_mon) <= tm.yday {
            t_mon += 1;
        }
        if !have_mon {
            tm.mon = t_mon - 1;
        }
        if !have_mday {
            tm.mday = tm.yday - mon_yday(l, t_mon - 1) + 1;
        }
    };
    let in_range = |tm: &Tm| (0..=11).contains(&tm.mon);
    if s.want_xday && !s.have_wday {
        if !(s.have_mon && s.have_mday) && s.have_yday {
            months(tm, s.have_mon, s.have_mday);
            s.have_mon = true;
            s.have_mday = true;
        }
        if s.have_mon || in_range(tm) {
            day_of_the_week(tm);
        }
    }
    if s.want_xday && !s.have_yday && (s.have_mon || in_range(tm)) {
        day_of_the_year(tm);
    }
    if (s.have_uweek || s.have_wweek) && s.have_wday {
        let (wday, mday, mon) = (tm.wday, tm.mday, tm.mon);
        let w_offset = i32::from(!s.have_uweek);
        tm.mday = 1;
        tm.mon = 0;
        day_of_the_week(tm);
        if s.have_mday {
            tm.mday = mday;
        }
        if s.have_mon {
            tm.mon = mon;
        }
        if !s.have_yday {
            tm.yday =
                (7 - (tm.wday - w_offset)) % 7 + (s.week_no - 1) * 7 + (wday - w_offset + 7) % 7;
        }
        if !s.have_mday || !s.have_mon {
            months(tm, s.have_mon, s.have_mday);
        }
        tm.wday = wday;
    }
    Some(rp)
}

/// `day_of_the_week`, C's truncating division kept, and its `int`
/// arithmetic wrapping as compiled (a year from `%s` can be near
/// `INT_MAX`).
fn day_of_the_week(tm: &mut Tm) {
    let w = Wrapping;
    let year = w(tm.year);
    let corr_year = w(1900) + year - w(i32::from(tm.mon < 2));
    let q = corr_year / w(4);
    let wday = w(-473) + w(365) * (year - w(70)) + q - q / w(25)
        + w(i32::from((q % w(25)).0 < 0))
        + q / w(25) / w(4)
        + w(mon_yday(0, tm.mon))
        + w(tm.mday)
        - w(1);
    tm.wday = ((wday % w(7) + w(7)) % w(7)).0;
}

/// `day_of_the_year`.
fn day_of_the_year(tm: &mut Tm) {
    let l = usize::from(is_leap(i64::from(tm.year) + 1900));
    tm.yday = mon_yday(l, tm.mon).wrapping_add(tm.mday).wrapping_sub(1);
}

/// `get_number`: spaces, then up to `n` digits while they stay within `to`.
fn number(input: &[u8], mut rp: usize, from: i32, to: i32, n: usize) -> Option<(i32, usize)> {
    while rp < input.len() && is_space(input[rp]) {
        rp += 1;
    }
    if !input.get(rp).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let mut val: i32 = 0;
    let mut left = n;
    loop {
        val = val * 10 + i32::from(input[rp] - b'0');
        rp += 1;
        left -= 1;
        if !(left > 0 && val * 10 <= to && input.get(rp).is_some_and(u8::is_ascii_digit)) {
            break;
        }
    }
    (from..=to).contains(&val).then_some((val, rp))
}

/// `match_string`: `name` at `rp`, case ignored.
fn matches_at(input: &[u8], rp: usize, name: &str) -> Option<usize> {
    let rest = input.get(rp..)?;
    (rest.len() >= name.len() && rest[..name.len()].eq_ignore_ascii_case(name.as_bytes()))
        .then_some(rp + name.len())
}

/// Whether a match ending at `trp` is longer than the longest so far
/// (`trp > rp_longest`, `NULL` the shortest).
fn longer(trp: usize, longest: Option<usize>) -> bool {
    longest.map_or(true, |l| trp > l)
}

/// `%a`: the longest day name at `rp`. glibc's fallback to the C names
/// matches an abbreviation against `rp` itself, moving it, so a later
/// day's name is looked for after an earlier one (`SunMon` reads as
/// Monday) and, once `raw`, an abbreviation alone reads nothing.
fn weekday(input: &[u8], mut rp: usize, s: &Parse) -> Option<(i32, usize)> {
    let mut longest: Option<usize> = None;
    let mut cnt_longest = -1;
    for (cnt, full) in (0..).zip(DAYS) {
        let abbr = &full[..3];
        if s.decided != Decided::Raw {
            for name in [full, abbr] {
                if let Some(trp) = matches_at(input, rp, name).filter(|&trp| longer(trp, longest)) {
                    longest = Some(trp);
                    cnt_longest = cnt;
                }
            }
        }
        if s.decided == Decided::Loc {
            continue;
        }
        let by_full = matches_at(input, rp, full).filter(|&trp| longer(trp, longest));
        let hit = by_full.or_else(|| {
            let trp = rp;
            let moved = matches_at(input, rp, abbr)?;
            rp = moved;
            longer(trp, longest).then_some(trp)
        });
        if let Some(trp) = hit {
            longest = Some(trp);
            cnt_longest = cnt;
        }
    }
    longest.map(|l| (cnt_longest, l))
}

/// `%b`: the longest month name at `rp`.
fn month(input: &[u8], rp: usize) -> Option<(i32, usize)> {
    let mut longest: Option<(i32, usize)> = None;
    for (cnt, full) in (0..).zip(MONTHS) {
        for name in [full, &full[..3]] {
            if let Some(trp) =
                matches_at(input, rp, name).filter(|&trp| longer(trp, longest.map(|(_, l)| l)))
            {
                longest = Some((cnt, trp));
            }
        }
    }
    longest
}

/// `recursive`: a format within the format, its changes kept only when
/// it matches.
fn recursive(input: &[u8], rp: usize, fmt: &str, tm: &mut Tm, s: &mut Parse) -> Option<usize> {
    let (mut t, mut st) = (tm.clone(), s.clone());
    let rp = parse(input, rp, fmt.as_bytes(), &mut t, &mut st)?;
    (*tm, *s) = (t, st);
    Some(rp)
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per conversion, as glibc's `__strptime_internal`"
)]
fn parse(input: &[u8], mut rp: usize, fmt: &[u8], tm: &mut Tm, s: &mut Parse) -> Option<usize> {
    let mut f = 0;
    while f < fmt.len() {
        if is_space(fmt[f]) {
            while rp < input.len() && is_space(input[rp]) {
                rp += 1;
            }
            f += 1;
            continue;
        }
        if fmt[f] != b'%' {
            if input.get(rp) != Some(&fmt[f]) {
                return None;
            }
            rp += 1;
            f += 1;
            continue;
        }
        f += 1;
        // strftime's flags and field width are dropped.
        while matches!(fmt.get(f), Some(b'-' | b'_' | b'0' | b'^' | b'#')) {
            f += 1;
        }
        while fmt.get(f).is_some_and(u8::is_ascii_digit) {
            f += 1;
        }
        let mut conv = *fmt.get(f)?;
        f += 1;
        // `%E` and `%O` before the conversions they allow; the C locale
        // has no eras and no alternative digits.
        let (mut era, mut alt) = (false, false);
        if conv == b'E' {
            conv = *fmt.get(f)?;
            f += 1;
            if !matches!(conv, b'c' | b'C' | b'y' | b'Y' | b'x' | b'X') {
                return None;
            }
            era = true;
        } else if conv == b'O' {
            conv = *fmt.get(f)?;
            f += 1;
            match conv {
                b'b' | b'B' | b'h' => {}
                b'd' | b'e' | b'H' | b'I' | b'm' | b'M' | b'S' | b'U' | b'W' | b'V' | b'w'
                | b'y' => {
                    // `get_alt_number`: no alternative digits, so the
                    // first decides for the locale and reads plain ones,
                    // and every later one fails.
                    match s.decided {
                        Decided::Not => s.decided = Decided::Loc,
                        Decided::Loc => return None,
                        Decided::Raw => {}
                    }
                    alt = true;
                }
                _ => return None,
            }
        }
        // An era conversion: the C locale has none, which fails once the
        // locale is decided on and else decides for the C locale.
        if era && matches!(conv, b'C' | b'y' | b'Y') {
            match s.decided {
                Decided::Loc => return None,
                Decided::Not if conv == b'y' => {
                    // `%Ey` reads a year of up to four digits as the
                    // era's first, then the year in the century after it.
                    let (v, r) = number(input, rp, 0, 9999, 4)?;
                    tm.year = v;
                    rp = r;
                    s.want_era = true;
                    s.want_xday = true;
                    s.want_century = true;
                    s.decided = Decided::Raw;
                }
                _ => s.decided = Decided::Raw,
            }
        }
        // The locale's own formats, which are the C locale's: if one
        // fails before the locale is decided, it is read again as the C
        // locale's, with `raw` decided.
        let sub = |fmt: &str, rp: usize, tm: &mut Tm, s: &mut Parse| {
            recursive(input, rp, fmt, tm, s).or_else(|| {
                if s.decided == Decided::Loc {
                    return None;
                }
                s.decided = Decided::Raw;
                recursive(input, rp, fmt, tm, s)
            })
        };
        match conv {
            b'%' => {
                if input.get(rp) != Some(&b'%') {
                    return None;
                }
                rp += 1;
            }
            b'a' | b'A' => {
                let (v, r) = weekday(input, rp, s)?;
                tm.wday = v;
                rp = r;
                s.have_wday = true;
            }
            b'b' | b'B' | b'h' => {
                let (v, r) = month(input, rp)?;
                tm.mon = v;
                rp = r;
                s.have_mon = true;
                s.want_xday = true;
            }
            b'c' => {
                rp = sub("%a %b %e %H:%M:%S %Y", rp, tm, s)?;
                s.want_xday = true;
            }
            b'C' => {
                let (v, r) = number(input, rp, 0, 99, 2)?;
                s.century = v;
                rp = r;
                s.want_xday = true;
            }
            b'd' | b'e' => {
                let (v, r) = number(input, rp, 1, 31, 2)?;
                tm.mday = v;
                rp = r;
                s.have_mday = true;
                s.want_xday = true;
            }
            b'F' => {
                rp = recursive(input, rp, "%Y-%m-%d", tm, s)?;
                s.want_xday = true;
            }
            b'x' => {
                rp = sub("%m/%d/%y", rp, tm, s)?;
                s.want_xday = true;
            }
            b'D' => {
                rp = recursive(input, rp, "%m/%d/%y", tm, s)?;
                s.want_xday = true;
            }
            b'k' | b'H' => {
                let (v, r) = number(input, rp, 0, 23, 2)?;
                tm.hour = v;
                rp = r;
                s.have_i = false;
            }
            b'l' | b'I' => {
                let (v, r) = number(input, rp, 1, 12, 2)?;
                tm.hour = v % 12;
                rp = r;
                s.have_i = true;
            }
            b'j' => {
                let (v, r) = number(input, rp, 1, 366, 3)?;
                tm.yday = v - 1;
                rp = r;
                s.have_yday = true;
            }
            b'm' => {
                let (v, r) = number(input, rp, 1, 12, 2)?;
                tm.mon = v - 1;
                rp = r;
                s.have_mon = true;
                s.want_xday = true;
            }
            b'M' => {
                let (v, r) = number(input, rp, 0, 59, 2)?;
                tm.min = v;
                rp = r;
            }
            b'n' | b't' => {
                while rp < input.len() && is_space(input[rp]) {
                    rp += 1;
                }
            }
            b'p' => {
                let am = matches_at(input, rp, "AM");
                s.is_pm = am.is_none();
                rp = am.or_else(|| matches_at(input, rp, "PM"))?;
            }
            b'r' => rp = sub("%I:%M:%S %p", rp, tm, s)?,
            b'R' => rp = recursive(input, rp, "%H:%M", tm, s)?,
            b's' => {
                // Digits only, into a `time_t` that wraps.
                if !input.get(rp).is_some_and(u8::is_ascii_digit) {
                    return None;
                }
                let mut secs: i64 = 0;
                while let Some(d) = input.get(rp).filter(|c| c.is_ascii_digit()) {
                    secs = secs.wrapping_mul(10).wrapping_add(i64::from(d - b'0'));
                    rp += 1;
                }
                *tm = localtime(secs)?;
            }
            b'S' => {
                let (v, r) = number(input, rp, 0, 61, 2)?;
                tm.sec = v;
                rp = r;
            }
            b'X' => rp = sub("%H:%M:%S", rp, tm, s)?,
            b'T' => rp = recursive(input, rp, "%H:%M:%S", tm, s)?,
            b'u' => {
                let (v, r) = number(input, rp, 1, 7, 1)?;
                tm.wday = v % 7;
                rp = r;
                s.have_wday = true;
            }
            b'g' => rp = number(input, rp, 0, 99, 2)?.1,
            b'G' => {
                if !input.get(rp).is_some_and(u8::is_ascii_digit) {
                    return None;
                }
                while input.get(rp).is_some_and(u8::is_ascii_digit) {
                    rp += 1;
                }
            }
            b'U' | b'W' => {
                let (v, r) = number(input, rp, 0, 53, 2)?;
                s.week_no = v;
                if conv == b'U' {
                    s.have_uweek = true;
                } else {
                    s.have_wweek = true;
                }
                rp = r;
            }
            b'V' => rp = number(input, rp, 0, 53, 2)?.1,
            b'w' => {
                let (v, r) = number(input, rp, 0, 6, 1)?;
                tm.wday = v;
                rp = r;
                s.have_wday = true;
            }
            b'y' => {
                let (v, r) = number(input, rp, 0, 99, 2)?;
                tm.year = if v >= 69 { v } else { v + 100 };
                rp = r;
                // `%Oy` alone leaves the century as it is.
                if !alt {
                    s.want_century = true;
                }
                s.want_xday = true;
            }
            b'Y' => {
                let (v, r) = number(input, rp, 0, 9999, 4)?;
                tm.year = v - 1900;
                rp = r;
                s.want_century = false;
                s.want_xday = true;
            }
            b'Z' => {
                while rp < input.len() && is_space(input[rp]) {
                    rp += 1;
                }
                while rp < input.len() && !is_space(input[rp]) {
                    rp += 1;
                }
            }
            b'z' => {
                while rp < input.len() && is_space(input[rp]) {
                    rp += 1;
                }
                if input.get(rp) == Some(&b'Z') {
                    rp += 1;
                    tm.gmtoff = 0;
                } else {
                    let neg = match input.get(rp) {
                        Some(b'+') => false,
                        Some(b'-') => true,
                        _ => return None,
                    };
                    rp += 1;
                    let mut n = 0;
                    let mut val = 0i64;
                    while n < 4 && input.get(rp).is_some_and(u8::is_ascii_digit) {
                        val = val * 10 + i64::from(input[rp] - b'0');
                        rp += 1;
                        n += 1;
                        if n == 2
                            && input.get(rp) == Some(&b':')
                            && input.get(rp + 1).is_some_and(u8::is_ascii_digit)
                        {
                            rp += 1;
                        }
                    }
                    if n == 2 {
                        val *= 100;
                    } else if n != 4 || val % 100 >= 60 {
                        return None;
                    }
                    let off = (val / 100) * 3600 + (val % 100) * 60;
                    tm.gmtoff = if neg { -off } else { off };
                }
            }
            _ => return None,
        }
    }
    Some(rp)
}

/// jq's `set_tm_wday` (Gauss's algorithm), for what `strptime` left unset;
/// its `int` arithmetic wrapping as compiled.
#[expect(clippy::cast_possible_truncation, reason = "C's `(int)floor(…)`")]
pub fn set_tm_wday(tm: &mut Tm) {
    let w = Wrapping;
    let full = w(1900) + w(tm.year);
    let century = full / w(100);
    let mut year = full % w(100);
    if tm.mon < 2 {
        year -= w(1);
    }
    let mut mon = w(tm.mon) - w(1);
    if mon.0 < 1 {
        mon += w(12);
    }
    let mut wday = (w(tm.mday)
        + w((2.6 * f64::from(mon.0) - 0.2).floor() as i32)
        + year
        + w((f64::from(year.0) / 4.0).floor() as i32)
        + w((f64::from(century.0) / 4.0).floor() as i32)
        - w(2) * century)
        % w(7);
    if wday.0 < 0 {
        wday += w(7);
    }
    tm.wday = wday.0;
}

/// jq's `set_tm_yday`.
pub fn set_tm_yday(tm: &mut Tm) {
    const D: [i32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let year = tm.year.wrapping_add(1900);
    let leap = i32::from(tm.mon > 1 && ((year % 4 == 0 && year % 100 != 0) || year % 400 == 0));
    // jq bounds the index: `-mon`, then `% 12`.
    let mon = usize::try_from(tm.mon.wrapping_abs() % 12).unwrap_or(0);
    tm.yday = D[mon]
        .wrapping_add(leap)
        .wrapping_add(tm.mday)
        .wrapping_sub(1);
}
