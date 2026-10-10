//! Reminder times: the text a person types (`today at 1`, `fri 9am`,
//! `in 2h`, `2026-10-14 09:00`) and the calendar picker's value.
use chrono::{
    DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike,
    Weekday,
};

const DEFAULT_HOUR: u32 = 9;

fn at(date: NaiveDate, time: NaiveTime) -> Option<DateTime<Local>> {
    Local.from_local_datetime(&date.and_time(time)).earliest()
}

fn weekday(word: &str) -> Option<Weekday> {
    Some(match word {
        "mon" | "monday" => Weekday::Mon,
        "tue" | "tues" | "tuesday" => Weekday::Tue,
        "wed" | "wednesday" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" => Weekday::Thu,
        "fri" | "friday" => Weekday::Fri,
        "sat" | "saturday" => Weekday::Sat,
        "sun" | "sunday" => Weekday::Sun,
        _ => return None,
    })
}

/// A clock time: `13:30`, `1:30pm`, `5pm`, `noon`, `midnight`, or a bare
/// hour. A bare hour 1-12 is ambiguous; `bare` decides how to read it.
fn clock(text: &str, bare: impl Fn(u32) -> u32) -> Option<NaiveTime> {
    let t = text.trim();
    match t {
        "noon" => return NaiveTime::from_hms_opt(12, 0, 0),
        "midnight" => return NaiveTime::from_hms_opt(0, 0, 0),
        _ => {}
    }
    let (body, meridiem) = match (t.strip_suffix("pm"), t.strip_suffix("am")) {
        (Some(b), _) => (b.trim(), Some(true)),
        (_, Some(b)) => (b.trim(), Some(false)),
        _ => (t, None),
    };
    let (hour, minute) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (body.parse::<u32>().ok()?, 0),
    };
    if minute > 59 {
        return None;
    }
    let hour = match meridiem {
        Some(pm) if (1..=12).contains(&hour) => hour % 12 + if pm { 12 } else { 0 },
        Some(_) => return None,
        None if body.contains(':') || hour == 0 || hour > 12 => hour,
        None => bare(hour),
    };
    NaiveTime::from_hms_opt(hour, minute, 0)
}

/// Parse a reminder time. Accepts a day (`today`, `tomorrow`, a weekday,
/// `YYYY-MM-DD`), an optional `at`, and a time; either may stand alone
/// (a day alone is 9:00, a time alone is the next one on the clock). Bare
/// hours 1-6 mean the afternoon, so `tomorrow at 1` is 13:00; for `today`
/// the next future reading of a bare hour is used. Also `in 30m`, `in 2h`,
/// `in 3 days` and `YYYY-MM-DD HH:MM`.
pub fn parse_when(input: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let lower = input.trim().to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("in ") {
        let rest = rest.trim();
        let (num, unit) = rest.split_at(rest.find(|c: char| !c.is_ascii_digit())?);
        let n: i64 = num.parse().ok()?;
        let d = match unit.trim() {
            "m" | "min" | "mins" | "minute" | "minutes" => Duration::minutes(n),
            "h" | "hr" | "hrs" | "hour" | "hours" => Duration::hours(n),
            "d" | "day" | "days" => Duration::days(n),
            "w" | "week" | "weeks" => Duration::weeks(n),
            _ => return None,
        };
        return Some(now + d);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(&lower, "%Y-%m-%d %H:%M") {
        return Local.from_local_datetime(&dt).earliest();
    }
    let mut words: Vec<&str> = lower.split_whitespace().collect();
    words.retain(|w| *w != "at" && *w != "on" && *w != "next");
    let today = now.date_naive();
    let (date, rest): (Option<NaiveDate>, &[&str]) = match words.split_first() {
        Some((&"today", rest)) => (Some(today), rest),
        Some((&"tomorrow", rest)) => (Some(today + Duration::days(1)), rest),
        Some((word, rest)) => {
            if let Some(day) = weekday(word) {
                let ahead = (7 + day.num_days_from_monday() as i64
                    - today.weekday().num_days_from_monday() as i64)
                    % 7;
                (
                    Some(today + Duration::days(if ahead == 0 { 7 } else { ahead })),
                    rest,
                )
            } else if let Ok(date) = NaiveDate::parse_from_str(word, "%Y-%m-%d") {
                (Some(date), rest)
            } else {
                (None, &words[..])
            }
        }
        None => return None,
    };
    let time_text = rest.join(" ");
    match date {
        Some(date) if time_text.is_empty() => {
            at(date, NaiveTime::from_hms_opt(DEFAULT_HOUR, 0, 0)?)
        }
        Some(date) if date == today => {
            // Today: a bare hour is the next one that is still ahead.
            let ahead = |h: u32| {
                [h, (h + 12) % 24]
                    .into_iter()
                    .filter_map(|c| NaiveTime::from_hms_opt(c, 0, 0))
                    .filter_map(|t| at(date, t))
                    .filter(|t| *t > now)
                    .min()
                    .map_or(h, |t| t.hour())
            };
            at(date, clock(&time_text, ahead)?)
        }
        Some(date) => at(date, clock(&time_text, |h| if h < 7 { h + 12 } else { h })?),
        None => {
            let time = clock(&time_text, |h| {
                // Next occurrence on the clock, reading am before pm.
                let first = h % 12;
                let candidate = |c: u32| at(today, NaiveTime::from_hms_opt(c, 0, 0)?);
                if candidate(first).is_some_and(|t| t > now) {
                    first
                } else if candidate(first + 12).is_some_and(|t| t > now) {
                    first + 12
                } else {
                    first
                }
            })?;
            let this = at(today, time)?;
            Some(if this > now {
                this
            } else {
                at(today + Duration::days(1), time)?
            })
        }
    }
}

/// The calendar picker's value: a date and a time on the hour grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatePicker {
    pub date: NaiveDate,
    pub hour: u32,
    pub minute: u32,
}

impl DatePicker {
    /// Starts at the next full hour.
    pub fn new(now: DateTime<Local>) -> Self {
        let next = (now + Duration::hours(1))
            .with_minute(0)
            .and_then(|t| t.with_second(0))
            .unwrap_or(now);
        Self {
            date: next.date_naive(),
            hour: next.hour(),
            minute: 0,
        }
    }

    pub fn add_days(&mut self, days: i64) {
        self.date += Duration::days(days);
    }

    pub fn add_months(&mut self, months: i32) {
        let index = self.date.year() * 12 + self.date.month0() as i32 + months;
        let (year, month) = (index.div_euclid(12), index.rem_euclid(12) as u32 + 1);
        let mut day = self.date.day();
        loop {
            if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
                self.date = date;
                return;
            }
            day -= 1;
        }
    }

    /// Move the time, rolling into the next or previous day.
    pub fn add_minutes(&mut self, minutes: i64) {
        let total = (self.hour as i64 * 60 + self.minute as i64 + minutes).rem_euclid(24 * 60);
        let days = (self.hour as i64 * 60 + self.minute as i64 + minutes).div_euclid(24 * 60);
        self.date += Duration::days(days);
        self.hour = (total / 60) as u32;
        self.minute = (total % 60) as u32;
    }

    pub fn datetime(&self) -> Option<DateTime<Local>> {
        at(
            self.date,
            NaiveTime::from_hms_opt(self.hour, self.minute, 0)?,
        )
    }

    /// Weeks of the shown month, Monday first.
    pub fn weeks(&self) -> Vec<[Option<u32>; 7]> {
        let first =
            NaiveDate::from_ymd_opt(self.date.year(), self.date.month(), 1).unwrap_or(self.date);
        let lead = first.weekday().num_days_from_monday() as usize;
        let mut days: Vec<Option<u32>> = vec![None; lead];
        let mut d = first;
        while d.month() == first.month() {
            days.push(Some(d.day()));
            d += Duration::days(1);
        }
        days.chunks(7)
            .map(|c| {
                let mut week = [None; 7];
                week[..c.len()].copy_from_slice(c);
                week
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Local> {
        // Saturday 10 October 2026, 14:30.
        Local.with_ymd_and_hms(2026, 10, 10, 14, 30, 0).unwrap()
    }

    fn when(text: &str) -> (u32, u32, u32, u32) {
        let t = parse_when(text, now()).unwrap_or_else(|| panic!("{text}"));
        (t.month(), t.day(), t.hour(), t.minute())
    }

    #[test]
    fn relative_and_absolute_times() {
        let n = now();
        assert_eq!(parse_when("in 2h", n), Some(n + Duration::hours(2)));
        assert_eq!(parse_when("in 30m", n), Some(n + Duration::minutes(30)));
        assert_eq!(parse_when("in 2 days", n), Some(n + Duration::days(2)));
        assert_eq!(when("tomorrow 9am"), (10, 11, 9, 0));
        assert_eq!(when("2026-10-14 09:00"), (10, 14, 9, 0));
        assert_eq!(when("2026-10-14"), (10, 14, 9, 0));
        // A time already past today means tomorrow.
        assert_eq!(when("09:00"), (10, 11, 9, 0));
        assert_eq!(when("5pm"), (10, 10, 17, 0));
        assert_eq!(parse_when("whenever", n), None);
        assert_eq!(parse_when("", n), None);
    }

    #[test]
    fn natural_days_and_bare_hours() {
        // A bare hour today is the next reading still ahead.
        assert_eq!(when("today at 5"), (10, 10, 17, 0));
        assert_eq!(when("today 3:45pm"), (10, 10, 15, 45));
        assert_eq!(when("today at 18:00"), (10, 10, 18, 0));
        assert_eq!(when("tomorrow at 1"), (10, 11, 13, 0));
        assert_eq!(when("tomorrow at 8"), (10, 11, 8, 0));
        assert_eq!(when("tomorrow noon"), (10, 11, 12, 0));
        assert_eq!(when("tomorrow"), (10, 11, 9, 0));
        assert_eq!(when("fri 9am"), (10, 16, 9, 0));
        assert_eq!(when("next monday at 10:15"), (10, 12, 10, 15));
        // The same weekday means next week.
        assert_eq!(when("saturday"), (10, 17, 9, 0));
        // A bare hour alone is the next one on the clock.
        assert_eq!(when("at 6"), (10, 10, 18, 0));
        assert_eq!(when("at 1"), (10, 11, 1, 0));
        assert_eq!(when("  Tomorrow   AT  9AM "), (10, 11, 9, 0));
    }

    #[test]
    fn picker_moves_by_day_month_and_time() {
        let mut p = DatePicker::new(now());
        assert_eq!((p.date.day(), p.hour, p.minute), (10, 15, 0));
        p.add_days(25);
        assert_eq!((p.date.month(), p.date.day()), (11, 4));
        p.date = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        p.add_months(1);
        assert_eq!((p.date.month(), p.date.day()), (2, 28));
        p.add_months(-2);
        assert_eq!((p.date.year(), p.date.month()), (2025, 12));
        p.hour = 23;
        p.minute = 30;
        p.add_minutes(45);
        assert_eq!((p.date.day(), p.hour, p.minute), (29, 0, 15));
        p.add_minutes(-30);
        assert_eq!((p.date.day(), p.hour, p.minute), (28, 23, 45));
        assert!(p.datetime().is_some());
    }

    #[test]
    fn month_grid_starts_on_monday() {
        let mut p = DatePicker::new(now());
        p.date = NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();
        let weeks = p.weeks();
        // 1 October 2026 is a Thursday.
        assert_eq!(
            weeks[0],
            [None, None, None, Some(1), Some(2), Some(3), Some(4)]
        );
        assert_eq!(weeks.last().unwrap()[0], Some(26));
        assert_eq!(weeks.iter().flatten().flatten().count(), 31);
    }
}
