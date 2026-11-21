//! Calendar dates as (year, month, day), and the wall clock.

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

pub fn now_secs() -> i64 { now_ms() / 1000 }

/// "9:41", or on a 12-hour clock "9:41" for 21:41 too.
pub fn clock(hour: u32, minute: u32, hour12: bool) -> String {
    format!("{}:{minute:02}", if hour12 { (hour + 11) % 12 + 1 } else { hour })
}

/// "2026-11-20" as (year, month, day); None for anything else or a day the month lacks.
pub fn parse_ymd(s: &str) -> Option<(i32, u32, u32)> {
    let mut p = s.splitn(3, '-').map(str::parse::<u32>);
    let (y, m, d) = (p.next()?.ok()? as i32, p.next()?.ok()?, p.next()?.ok()?);
    ((1..=12).contains(&m) && (1..=days_in(y, m)).contains(&d)).then_some((y, m, d))
}

/// `2026-11-20T07:12` as seconds since 1970 on that clock.
pub fn local_secs(t: &str) -> Option<i64> {
    let (date, time) = t.split_once('T')?;
    let (y, m, d) = parse_ymd(date)?;
    let (h, min) = time.split_once(':')?;
    Some(days_from_civil(y, m, d) * 86_400 + i64::from(h.parse::<u32>().ok()? * 3600 + min.get(..2)?.parse::<u32>().ok()? * 60))
}

/// Each weekday's initial, from Sunday.
pub const WEEKDAY_LETTERS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];

/// "Thu".
pub fn weekday_name(year: i32, month: u32, day: u32) -> &'static str {
    ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][weekday(year, month, day) as usize]
}

/// "Jan 1".
pub fn month_day(month: u32, day: u32) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {day}", MONTHS[(month as usize - 1) % 12])
}

/// "Thu, Jan 1".
pub fn format_date(year: i32, month: u32, day: u32) -> String { format!("{}, {}", weekday_name(year, month, day), month_day(month, day)) }

/// 0 for Sunday, by Sakamoto's method.
pub fn weekday(year: i32, month: u32, day: u32) -> u32 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    (y + y / 4 - y / 100 + y / 400 + T[(month as usize - 1) % 12] + day as i32).rem_euclid(7) as u32
}

pub fn days_in(year: i32, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

pub fn shift_month(year: i32, month: u32, offset: i32) -> (i32, u32) {
    let i = year * 12 + month as i32 - 1 + offset;
    (i.div_euclid(12), i.rem_euclid(12) as u32 + 1)
}

pub fn utc_days_ago(days: i64) -> rune::time::LocalTime { rune::time::LocalTime::at_offset(now_ms() - days * 86_400_000, 0) }

pub fn add_days((y, m, d): (i32, u32, u32), days: i64) -> (i32, u32, u32) {
    let t = rune::time::LocalTime::at_offset((days_from_civil(y, m, d) + days) * 86_400_000, 0);
    (t.year, t.month, t.day)
}

/// Days since 1970-01-01, from Howard Hinnant's `days_from_civil`.
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((i64::from(m) + 9) % 12) + 2) / 5 + i64::from(d) - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read_like_the_short_weekday_month_day() {
        assert_eq!(format_date(2026, 1, 1), "Thu, Jan 1");
        assert_eq!(format_date(2024, 2, 29), "Thu, Feb 29");
        assert_eq!(format_date(2026, 11, 17), "Tue, Nov 17");
        assert_eq!(weekday(2026, 11, 18), 3);
    }

    #[test]
    fn dates_and_times_parse_only_when_real() {
        assert_eq!(parse_ymd("2024-02-29"), Some((2024, 2, 29)));
        assert_eq!((parse_ymd("2026-02-29"), parse_ymd("2026-13-01"), parse_ymd("today")), (None, None, None));
        assert_eq!(local_secs("1970-01-02T01:30"), Some(86_400 + 5400));
        assert_eq!((clock(21, 5, true), clock(0, 5, true), clock(21, 5, false)), ("9:05".into(), "12:05".into(), "21:05".into()));
    }

    #[test]
    fn months_page_across_years() {
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 11, -23), (2024, 12));
        assert_eq!((days_in(2024, 2), days_in(2026, 2), days_in(2000, 2), days_in(1900, 2)), (29, 28, 29, 28));
    }

    #[test]
    fn days_count_from_the_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        let t = rune::time::LocalTime::at_offset(days_from_civil(2024, 2, 29) * 86_400_000, 0);
        assert_eq!((t.year, t.month, t.day), (2024, 2, 29));
    }
}
