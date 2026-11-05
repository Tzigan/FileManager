use std::io::{self, Read, Write};
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};

/// Named pipe used to forward CLI paths to the already-running instance.
const PIPE: &str = r"\\.\pipe\FileManagerArgs";

/// Primary instance: listen for forwarded paths on a dedicated thread.
pub fn listen(on_path: impl FnMut(PathBuf) + Send + 'static) {
    std::thread::spawn(move || listen_loop(on_path));
}

fn listen_loop(mut on_path: impl FnMut(PathBuf)) {
    use ::windows::core::{HSTRING, PCWSTR};
    use ::windows::Win32::Foundation::HANDLE;
    use ::windows::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
    };
    use ::windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE, PIPE_WAIT,
    };
    loop {
        unsafe {
            let name = HSTRING::from(PIPE);
            let h = CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                FILE_FLAG_FIRST_PIPE_INSTANCE | PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                1,
                64 * 1024,
                64 * 1024,
                0,
                None,
            );
            if h == HANDLE::default() {
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
            let _ = ConnectNamedPipe(h, None);
            let mut file = std::fs::File::from_raw_handle(h.0 as *mut _);
            let mut s = String::new();
            let _ = file.read_to_string(&mut s);
            let t = s.trim();
            if !t.is_empty() {
                on_path(PathBuf::from(t));
            }
        }
    }
}

/// Secondary instance: forward `path` to the primary and exit.
pub fn send(path: &Path) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).open(PIPE)?;
    f.write_all(path.as_os_str().to_string_lossy().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn pipe_forwards_path() {
        let (tx, rx) = channel();
        listen(move |p| {
            let _ = tx.send(p);
        });
        let mut sent = false;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(100));
            if send(Path::new(r"C:\test\pipe_path")).is_ok() {
                sent = true;
                break;
            }
        }
        assert!(sent, "send() never connected to the pipe");
        let got = rx.recv_timeout(Duration::from_secs(5));
        assert_eq!(got.unwrap(), PathBuf::from(r"C:\test\pipe_path"));
    }
}
