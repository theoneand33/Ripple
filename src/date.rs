//! Calendar dates as (year, month, day), and the wall clock.

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Unix seconds now.
pub fn now_secs() -> i64 { now_ms() / 1000 }

/// "Thu, Jan 1".
pub fn format_date(year: i32, month: u32, day: u32) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    format!("{}, {} {}", DAYS[weekday(year, month, day) as usize], MONTHS[(month as usize - 1) % 12], day)
}

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

/// The month `offset` months from (year, month).
pub fn shift_month(year: i32, month: u32, offset: i32) -> (i32, u32) {
    let i = year * 12 + month as i32 - 1 + offset;
    (i.div_euclid(12), i.rem_euclid(12) as u32 + 1)
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
