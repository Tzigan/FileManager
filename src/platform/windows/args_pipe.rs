use std::io::{self, Read, Write};
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};

/// Named pipe used to forward CLI paths to the already-running instance.
const PIPE: &str = r"\\.\pipe\FileManagerArgs";

/// Primary instance: listen for forwarded paths on a dedicated thread.
pub fn listen(on_path: impl FnMut(PathBuf) + Send + 'static) {
    listen_named(PIPE, on_path);
}

fn listen_named(pipe: &'static str, on_path: impl FnMut(PathBuf) + Send + 'static) {
    std::thread::spawn(move || listen_loop(pipe, on_path));
}

fn listen_loop(pipe: &str, mut on_path: impl FnMut(PathBuf)) {
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
            let name = HSTRING::from(pipe);
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
    send_named(PIPE, path)
}

fn send_named(pipe: &str, path: &Path) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).open(pipe)?;
    f.write_all(path.as_os_str().to_string_lossy().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

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
