use std::io;
use std::path::PathBuf;

use ::windows::core::HSTRING;
use ::windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, COINIT_APARTMENTTHREADED, CLSCTX_ALL};
use ::windows::Win32::UI::Shell::{
    IFileOperation, IShellItem, SHCreateItemFromParsingName, FileOperation,
    FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT, FOFX_RECYCLEONDELETE,
};

/// Moves `paths` to the Windows Recycle Bin via IFileOperation.
/// Runs silently (no UI) on the calling thread — call from a worker.
pub fn recycle_bin(paths: &[PathBuf]) -> io::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.message()))?;
        op.SetOperationFlags(FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION | FOF_SILENT)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.message()))?;
        for p in paths {
            let name = HSTRING::from(p.as_os_str());
            let item: IShellItem = SHCreateItemFromParsingName(&name, None)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("{}: {}", p.display(), e.message())))?;
            op.DeleteItem(&item, None)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.message()))?;
        }
        op.PerformOperations()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.message()))?;
    }
    Ok(())
}
