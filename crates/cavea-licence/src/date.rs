//! Calendar dates, `YYYY-MM-DD` in UTC, as days since 1970-01-01 (12 §7.2).

use std::fmt;

/// Milliseconds in a day.
pub const DAY_MS: u64 = 86_400_000;

/// A day of the proleptic Gregorian calendar, 1970-01-01 to 9999-12-31.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(u32);

impl Date {
    /// `YYYY-MM-DD` exactly: four digits, two, two, a day that exists.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 10 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
            return None;
        }
        let digits = |range: std::ops::Range<usize>| -> Option<u32> {
            let part = b.get(range)?;
            part.iter().try_fold(0u32, |n, &c| {
                c.is_ascii_digit().then(|| n * 10 + u32::from(c - b'0'))
            })
        };
        let (year, month, day) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
        if year < 1970 || !(1..=12).contains(&month) || day == 0 || day > days_in(year, month) {
            return None;
        }
        Some(Self(days_from_civil(year, month, day)))
    }

    /// The day holding `ms` milliseconds since the epoch.
    #[must_use]
    pub fn of_ms(ms: u64) -> Self {
        Self(u32::try_from(ms / DAY_MS).unwrap_or(u32::MAX))
    }

    /// Days since 1970-01-01.
    #[must_use]
    pub fn days(self) -> u32 {
        self.0
    }

    /// 00:00:00 UTC of this day, in milliseconds since the epoch.
    #[must_use]
    pub fn start_ms(self) -> u64 {
        u64::from(self.0) * DAY_MS
    }

    /// The day `n` days later (saturating).
    #[must_use]
    pub fn plus(self, n: u32) -> Self {
        Self(self.0.saturating_add(n))
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = civil_from_days(self.0);
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

fn leap(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn days_in(year: u32, month: u32) -> u32 {
    match month {
        2 if leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// Howard Hinnant's algorithms, for years from 1970 (no negative values).
fn days_from_civil(year: u32, month: u32, day: u32) -> u32 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: u32) -> (u32, u32, u32) {
    let z = u64::from(days) + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    let narrow = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);
    (narrow(year), narrow(month), narrow(day))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::*;

    #[test]
    fn dates_parse_only_when_they_exist() {
        for ok in [
            "1970-01-01",
            "2027-01-10",
            "2028-02-29",
            "2000-02-29",
            "9999-12-31",
        ] {
            assert_eq!(Date::parse(ok).unwrap().to_string(), ok);
        }
        for bad in [
            "",
            "2027-1-10",
            "2027-01-10T00:00:00Z",
            "2027/01/10",
            "2027-00-10",
            "2027-13-01",
            "2027-02-29",
            "1900-02-29",
            "2027-04-31",
            "2027-01-00",
            "1969-12-31",
            "+027-01-10",
            "２０２７-01-10",
            " 2027-01-10",
        ] {
            assert_eq!(Date::parse(bad), None, "{bad}");
        }
        assert_eq!(Date::parse("1970-01-02").unwrap().days(), 1);
        assert_eq!(
            Date::parse("2027-01-10").unwrap().start_ms(),
            1_799_539_200_000
        );
    }

    #[test]
    fn every_day_round_trips() {
        for days in 0..(10_000 * 366) {
            let date = Date(days);
            let text = date.to_string();
            if text.len() > 10 {
                break;
            }
            assert_eq!(Date::parse(&text), Some(date), "{text}");
        }
    }
}
