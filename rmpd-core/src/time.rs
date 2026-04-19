/// Shared time utilities: Unix timestamp conversion and ISO 8601 formatting.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Convert a SystemTime to Unix timestamp (seconds since epoch).
pub fn system_time_to_unix_secs(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| {
            tracing::warn!("system time before UNIX_EPOCH, using 0");
            Duration::ZERO
        })
        .as_secs() as i64
}

/// Convert Unix timestamp to ISO 8601 format (RFC 3339).
pub fn format_iso8601(timestamp: i64) -> String {
    const SECONDS_PER_MINUTE: i64 = 60;
    const SECONDS_PER_HOUR: i64 = 3600;
    const SECONDS_PER_DAY: i64 = 86400;

    let mut days = timestamp / SECONDS_PER_DAY;
    let remaining = timestamp % SECONDS_PER_DAY;
    let hours = remaining / SECONDS_PER_HOUR;
    let minutes = (remaining % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE;
    let seconds = remaining % SECONDS_PER_MINUTE;

    // Calculate year starting from 1970
    let mut year = 1970;
    loop {
        let leap_year = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let days_in_year = if leap_year { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    // Calculate month and day
    let leap_year = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let days_in_month = if leap_year {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &dim in &days_in_month {
        if days < dim {
            break;
        }
        days -= dim;
        month += 1;
    }
    let day = days + 1;

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}
