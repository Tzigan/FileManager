use std::io::{self, Read, Write};
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};

/// Named pipe used to forward CLI paths to the already-running instance.
const PIPE: &str = r"\\.\pipe\FileManagerArgs";
const MAX_PIPE_PATH_BYTES: u64 = 128 * 1024;

/// Primary instance: listen for forwarded paths on a dedicated thread.
pub fn listen(on_path: impl FnMut(PathBuf) + Send + 'static) {
    listen_named(PIPE, on_path);
}

fn listen_named(pipe: &'static str, on_path: impl FnMut(PathBuf) + Send + 'static) {
    std::thread::spawn(move || listen_loop(pipe, on_path));
}

fn listen_loop(pipe: &str, mut on_path: impl FnMut(PathBuf)) {
    use ::windows::core::{HSTRING, PCWSTR};
    use ::windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED};
    use ::windows::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
    };
    use ::windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    loop {
        unsafe {
            let name = HSTRING::from(pipe);
            let h = CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                FILE_FLAG_FIRST_PIPE_INSTANCE | PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                64 * 1024,
                64 * 1024,
                0,
                None,
            );
            if h.is_invalid() {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
            if let Err(error) = ConnectNamedPipe(h, None) {
                if (error.code().0 as u32 & 0xffff) != ERROR_PIPE_CONNECTED.0 {
                    let _ = CloseHandle(h);
                    continue;
                }
            }
            let file = std::fs::File::from_raw_handle(h.0 as *mut _);
            if let Ok(Some(path)) = read_pipe_path(file) {
                on_path(path);
            }
        }
    }
}

fn read_pipe_path(reader: impl Read) -> io::Result<Option<PathBuf>> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_PIPE_PATH_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PIPE_PATH_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "forwarded path exceeds the size limit",
        ));
    }
    let text = String::from_utf8(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let path = text.trim();
    if path.is_empty() || path.contains('\0') {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(path)))
}

/// Secondary instance: forward `path` to the primary and exit.
pub fn send(path: &Path) -> io::Result<()> {
    send_named(PIPE, path)
}

fn send_named(pipe: &str, path: &Path) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).open(pipe)?;
    f.write_all(path.as_os_str().to_string_lossy().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn pipe_payload_rejects_oversized_path() {
        let bytes = vec![b'x'; MAX_PIPE_PATH_BYTES as usize + 1];
        assert_eq!(
            read_pipe_path(Cursor::new(bytes)).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn pipe_forwards_path() {
        // Own pipe name: the test must not race a running FileManager instance.
        const TEST_PIPE: &str = r"\\.\pipe\FileManagerArgsTest";
        let (tx, rx) = channel();
        listen_named(TEST_PIPE, move |p| {
            let _ = tx.send(p);
        });
        let mut sent = false;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(100));
            if send_named(TEST_PIPE, Path::new(r"C:\test\pipe_path")).is_ok() {
                sent = true;
                break;
            }
        }
        assert!(sent, "send() never connected to the pipe");
        let got = rx.recv_timeout(Duration::from_secs(5));
        assert_eq!(got.unwrap(), PathBuf::from(r"C:\test\pipe_path"));
    }
}
