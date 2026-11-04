//! Formatting helpers shared by the UI layer.

/// Human-readable file size: `512 B`, `1.5 KB`, `2.3 MB`, `4 GB`.
/// Uses binary units (1024) matching Windows Explorer conventions.
pub fn format_size(size: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if size < 1024 {
        return format!("{size} B");
    }
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Formats a Unix timestamp (seconds since epoch) as `YYYY-MM-DD HH:MM`
/// in UTC. Used as a fallback when local-time conversion is unavailable.
pub fn format_unix_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60
    )
}

/// Days since 1970-01-01 → (year, month, day).
/// Howard Hinnant's civil_from_days algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(format_size(120 * 1024 * 1024), "120 MB");
    }

    #[test]
    fn formats_unix_utc() {
        assert_eq!(format_unix_utc(0), "1970-01-01 00:00");
        // 2000-02-29 12:34 UTC = 951827640
        assert_eq!(format_unix_utc(951_827_640), "2000-02-29 12:34");
        // 2026-11-04 00:00 UTC = 1793750400
        assert_eq!(format_unix_utc(1_793_750_400), "2026-11-04 00:00");
    }
}
