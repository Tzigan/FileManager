use std::time::SystemTime;

use ::windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use ::windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

/// 100-nanosecond intervals between 1601-01-01 and 1970-01-01.
const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;

/// Formats a `SystemTime` as `YYYY-MM-DD HH:MM` in the user's local timezone.
/// Falls back to UTC formatting for out-of-range or pre-1970 timestamps.
pub fn format_local_time(t: SystemTime) -> String {
    let Some(ft) = system_time_to_filetime(t) else {
        return "-".to_string();
    };

    unsafe {
        let mut utc = SYSTEMTIME::default();
        let mut local = SYSTEMTIME::default();
        if FileTimeToSystemTime(&ft, &mut utc).is_ok()
            && SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_ok()
        {
            return format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
            );
        }
    }

    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => crate::core::format::format_unix_utc(d.as_secs()),
        Err(_) => "-".to_string(),
    }
}

fn system_time_to_filetime(t: SystemTime) -> Option<FILETIME> {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    let ft = EPOCH_DIFF_100NS + d.as_secs() * 10_000_000 + u64::from(d.subsec_nanos()) / 100;
    Some(FILETIME {
        dwLowDateTime: ft as u32,
        dwHighDateTime: (ft >> 32) as u32,
    })
}
