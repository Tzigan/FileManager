use std::time::SystemTime;

use ::windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use ::windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use crate::core::i18n::Language;

/// 100-nanosecond intervals between 1601-01-01 and 1970-01-01.
const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;

/// Formats a `SystemTime` in the user's local timezone and date convention.
pub fn format_local_time(t: SystemTime, language: Language) -> String {
    let Some(ft) = system_time_to_filetime(t) else {
        return "-".to_string();
    };

    unsafe {
        let mut utc = SYSTEMTIME::default();
        let mut local = SYSTEMTIME::default();
        if FileTimeToSystemTime(&ft, &mut utc).is_ok()
            && SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_ok()
        {
            return crate::core::format::format_datetime_parts(
                i64::from(local.wYear),
                u32::from(local.wMonth),
                u32::from(local.wDay),
                u32::from(local.wHour),
                u32::from(local.wMinute),
                language,
            );
        }
    }

    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => crate::core::format::format_unix_utc_for_language(d.as_secs(), language),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_timestamp_has_documented_shape() {
        let timestamp = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let english = format_local_time(timestamp, Language::En);
        let russian = format_local_time(timestamp, Language::Ru);
        assert_eq!(english.len(), 16);
        assert_eq!(&english[4..5], "-");
        assert_eq!(&english[7..8], "-");
        assert_eq!(&english[10..11], " ");
        assert_eq!(&english[13..14], ":");
        assert_eq!(&russian[2..3], "-");
        assert_eq!(&russian[5..6], "-");
        assert_eq!(&russian[10..11], " ");
        assert_eq!(&russian[13..14], ":");
    }

    #[test]
    fn pre_epoch_timestamp_uses_documented_fallback() {
        assert_eq!(
            format_local_time(
                SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(1),
                Language::Ru,
            ),
            "-"
        );
    }
}
