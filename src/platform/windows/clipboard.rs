use std::io;
use std::mem::size_of;
use std::path::PathBuf;

use ::windows::Win32::Foundation::{GlobalFree, HANDLE};
use ::windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use ::windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GHND};
use ::windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};
use ::windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

const CF_HDROP: u32 = 15;
const CF_UNICODETEXT: u32 = 13;

fn hdrop_paths_wide(paths: &[PathBuf]) -> Vec<u16> {
    let mut wide = Vec::new();
    for path in paths {
        wide.extend(path.as_os_str().to_string_lossy().encode_utf16());
        wide.push(0);
    }
    if wide.is_empty() {
        wide.push(0);
    }
    wide.push(0);
    wide
}

fn hdrop_data(paths: &[PathBuf]) -> Vec<u8> {
    let wide = hdrop_paths_wide(paths);
    let header_size = size_of::<DROPFILES>();
    let mut data = vec![0u8; header_size + wide.len() * size_of::<u16>()];
    unsafe {
        let mut header: DROPFILES = std::mem::zeroed();
        header.pFiles = header_size as u32;
        header.fWide = true.into();
        std::ptr::write_unaligned(data.as_mut_ptr().cast::<DROPFILES>(), header);
        std::ptr::copy_nonoverlapping(
            wide.as_ptr(),
            data.as_mut_ptr().add(header_size).cast::<u16>(),
            wide.len(),
        );
    }
    data
}

fn decode_clipboard_utf16(units: &[u16]) -> String {
    let len = units.iter().position(|&unit| unit == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..len])
}

/// Places `paths` on the Windows clipboard as `CF_HDROP` (Explorer-compatible).
pub fn set_clipboard_files(paths: &[PathBuf]) -> io::Result<()> {
    unsafe {
        // Clipboard ownership must be associated with a real window. Passing
        // NULL can leave no owner and make SetClipboardData fail after clearing
        // the previous contents.
        let hwnd = GetForegroundWindow();
        let owner = (!hwnd.is_invalid()).then_some(hwnd);
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(owner).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if !opened {
            return Err(io::Error::last_os_error());
        }
        if let Err(e) = EmptyClipboard() {
            let _ = CloseClipboard();
            return Err(io::Error::new(io::ErrorKind::Other, e.message()));
        }

        let data = hdrop_data(paths);

        let h = match GlobalAlloc(GHND, data.len()) {
            Ok(h) => h,
            Err(e) => {
                let _ = CloseClipboard();
                return Err(io::Error::new(io::ErrorKind::Other, e.message()));
            }
        };
        let ptr = GlobalLock(h) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(h));
            let _ = CloseClipboard();
            return Err(io::Error::last_os_error());
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        let _ = GlobalUnlock(h);

        let ok = SetClipboardData(CF_HDROP, Some(HANDLE(h.0 as *mut _)));
        let _ = CloseClipboard();
        if let Err(e) = ok {
            let _ = GlobalFree(Some(h));
            return Err(io::Error::new(io::ErrorKind::Other, e.message()));
        }
    }
    Ok(())
}

/// Reads `CF_HDROP` paths from the clipboard (files copied in Explorer).
pub fn get_clipboard_files() -> Vec<PathBuf> {
    unsafe {
        if OpenClipboard(None).is_err() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Ok(h) = GetClipboardData(CF_HDROP) {
            let hdrop = HDROP(h.0);
            let n = DragQueryFileW(hdrop, u32::MAX, None);
            for i in 0..n {
                let len = DragQueryFileW(hdrop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(hdrop, i, Some(&mut buf));
                out.push(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
            }
        }
        let _ = CloseClipboard();
        out
    }
}

/// Reads Unicode text so Ctrl+V in text fields still works when eframe's
/// automatic clipboard-shortcut translation is disabled for file shortcuts.
pub fn get_clipboard_text() -> String {
    unsafe {
        if OpenClipboard(None).is_err() {
            return String::new();
        }
        let text = GetClipboardData(CF_UNICODETEXT)
            .ok()
            .and_then(|h| {
                let size = GlobalSize(::windows::Win32::Foundation::HGLOBAL(h.0));
                if size < 2 {
                    return None;
                }
                let ptr = GlobalLock(::windows::Win32::Foundation::HGLOBAL(h.0)) as *const u16;
                if ptr.is_null() {
                    return None;
                }
                let len = (size / 2).min(1_048_576);
                let slice = std::slice::from_raw_parts(ptr, len);
                let result = decode_clipboard_utf16(slice);
                let _ = GlobalUnlock(::windows::Win32::Foundation::HGLOBAL(h.0));
                Some(result)
            })
            .unwrap_or_default();
        let _ = CloseClipboard();
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdrop_payload_is_double_terminated_and_preserves_unicode_paths() {
        let paths = vec![PathBuf::from("C:\\данные\\a.txt"), PathBuf::from("D:\\b.txt")];
        let data = hdrop_data(&paths);
        let header_size = size_of::<DROPFILES>();
        let header = unsafe { std::ptr::read_unaligned(data.as_ptr().cast::<DROPFILES>()) };
        let file_offset = header.pFiles as usize;
        let wide_flag = header.fWide.0;
        assert_eq!(file_offset, header_size);
        assert_ne!(wide_flag, 0);
        assert_eq!(data.len(), header_size + hdrop_paths_wide(&paths).len() * 2);
        let wide = unsafe {
            std::slice::from_raw_parts(data.as_ptr().add(file_offset).cast::<u16>(), (data.len() - header_size) / 2)
        };
        assert_eq!(&wide[wide.len() - 2..], &[0u16, 0u16]);
        let decoded: Vec<String> = wide
            .split(|&unit| unit == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf16(part).unwrap())
            .collect();
        assert_eq!(decoded, vec!["C:\\данные\\a.txt", "D:\\b.txt"]);
    }

    #[test]
    fn empty_hdrop_list_still_has_required_double_terminator() {
        let data = hdrop_data(&[]);
        let header_size = size_of::<DROPFILES>();
        assert_eq!((data.len() - header_size) / 2, 2);
        let wide = unsafe {
            std::slice::from_raw_parts(data.as_ptr().add(header_size).cast::<u16>(), 2)
        };
        assert_eq!(wide, [0, 0]);
    }

    #[test]
    fn clipboard_text_decodes_unicode_and_stops_at_the_terminator() {
        let mut units: Vec<u16> = "путь 🚀".encode_utf16().collect();
        units.push(0);
        units.extend("ignored".encode_utf16());
        assert_eq!(decode_clipboard_utf16(&units), "путь 🚀");
        assert_eq!(decode_clipboard_utf16(&[]), "");
    }
}
