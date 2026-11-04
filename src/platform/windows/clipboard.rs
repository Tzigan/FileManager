use std::io;
use std::mem::size_of;
use std::path::PathBuf;

use ::windows::Win32::Foundation::HANDLE;
use ::windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use ::windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};
use ::windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};

const CF_HDROP: u32 = 15;

/// Places `paths` on the Windows clipboard as `CF_HDROP` (Explorer-compatible).
pub fn set_clipboard_files(paths: &[PathBuf]) -> io::Result<()> {
    unsafe {
        OpenClipboard(None).map_err(|e| io::Error::new(io::ErrorKind::Other, e.message()))?;
        let _ = EmptyClipboard();

        // DROPFILES header + UTF-16 paths joined with \0 + trailing \0\0.
        let mut wide: Vec<u16> = Vec::new();
        for p in paths {
            wide.extend(p.as_os_str().to_string_lossy().encode_utf16());
            wide.push(0);
        }
        wide.push(0);
        let bytes = size_of::<DROPFILES>() + wide.len() * 2;

        let h = GlobalAlloc(GHND, bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.message()))?;
        let ptr = GlobalLock(h) as *mut u8;
        let df = ptr as *mut DROPFILES;
        (*df).pFiles = size_of::<DROPFILES>() as u32;
        (*df).fWide = true.into();
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr.add(size_of::<DROPFILES>()) as *mut u16, wide.len());
        let _ = GlobalUnlock(h);

        let ok = SetClipboardData(CF_HDROP, Some(HANDLE(h.0 as *mut _)));
        let _ = CloseClipboard();
        ok.map_err(|e| io::Error::new(io::ErrorKind::Other, e.message()))?;
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
