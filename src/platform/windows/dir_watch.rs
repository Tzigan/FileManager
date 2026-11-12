//! Одно наблюдение за каталогом через `ReadDirectoryChangesW`.
//!
//! Дескриптор живёт только пока папка открыта. Смена папки закрывает предыдущий.
//! Имена из буфера в список не попадают: любое событие или обрыв буфера — повод
//! перечитать папку целиком.

use std::path::{Path, PathBuf};

use ::windows::Win32::Foundation::{
    CloseHandle, ERROR_NOTIFY_ENUM_DIR, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use ::windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use ::windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use ::windows::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};
use ::windows::core::HSTRING;

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

use crate::core::dir_watch::{DirWatchCommand, WatchNote, WatchSignal};

const BUFFER_BYTES: usize = 16 * 1024;
const POLL_MS: u32 = 50;

pub struct DirWatchSlot {
    directory: HANDLE,
    event: HANDLE,
    path: PathBuf,
    buffer: Vec<u8>,
    overlapped: OVERLAPPED,
    /// Чтение уже висит. Слот нельзя переносить, пока флаг установлен.
    armed: bool,
}

impl DirWatchSlot {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let directory = unsafe {
            CreateFileW(
                &HSTRING::from(path.as_os_str()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        let event = unsafe { CreateEventW(None, true, false, None) }
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(Self {
            directory,
            event,
            path: path.to_path_buf(),
            buffer: vec![0u8; BUFFER_BYTES],
            overlapped: OVERLAPPED::default(),
            armed: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `None` — за эту паузу ничего не случилось. Дескриптор остаётся открытым.
    pub fn poll(&mut self) -> std::io::Result<Option<WatchSignal>> {
        if !self.armed {
            self.overlapped = OVERLAPPED::default();
            self.overlapped.hEvent = self.event;
            let _ = unsafe { ResetEvent(self.event) };
            let filter = FILE_NOTIFY_CHANGE_FILE_NAME
                | FILE_NOTIFY_CHANGE_DIR_NAME
                | FILE_NOTIFY_CHANGE_LAST_WRITE;
            let started = unsafe {
                ReadDirectoryChangesW(
                    self.directory,
                    self.buffer.as_mut_ptr().cast(),
                    self.buffer.len() as u32,
                    false,
                    filter,
                    None,
                    Some(&mut self.overlapped),
                    None,
                )
            };
            if let Err(error) = started {
                // Отложенное чтение — это не ошибка, дескриптор уже ждёт событие.
                let code = error.code().0 as u32;
                if code != 997 {
                    return Ok(Some(signal_for_error(code)));
                }
            }
            self.armed = true;
        }
        let wait = unsafe { WaitForSingleObject(self.event, POLL_MS) };
        if wait == WAIT_TIMEOUT {
            return Ok(None);
        }
        if wait != WAIT_OBJECT_0 {
            self.armed = false;
            return Ok(Some(WatchSignal::Unavailable));
        }
        let mut transferred = 0u32;
        let finished =
            unsafe { GetOverlappedResult(self.directory, &self.overlapped, &mut transferred, false) };
        let _ = unsafe { ResetEvent(self.event) };
        self.armed = false;
        if let Err(error) = finished {
            return Ok(Some(signal_for_error(error.code().0 as u32)));
        }
        Ok(Some(signal_for_buffer(&self.buffer, transferred)))
    }
}

impl Drop for DirWatchSlot {
    fn drop(&mut self) {
        unsafe {
            if self.armed {
                let _ = CancelIoEx(self.directory, Some(&self.overlapped));
            }
            let _ = CloseHandle(self.directory);
            let _ = CloseHandle(self.event);
        }
    }
}

/// Один поток на все папки. Новая папка закрывает предыдущий дескриптор.
pub fn spawn_dir_watch(events: Sender<WatchNote>) -> Sender<DirWatchCommand> {
    let (commands_tx, commands_rx) = channel();
    thread::spawn(move || coordinate(commands_rx, events));
    commands_tx
}

fn coordinate(commands: Receiver<DirWatchCommand>, events: Sender<WatchNote>) {
    let mut slot: Option<DirWatchSlot> = None;
    loop {
        match commands.recv_timeout(Duration::from_millis(POLL_MS as u64)) {
            Ok(DirWatchCommand::Stop) => slot = None,
            Ok(DirWatchCommand::Watch(path)) => {
                if slot.as_ref().is_none_or(|current| current.path() != path) {
                    slot = match DirWatchSlot::open(&path) {
                        Ok(opened) => Some(opened),
                        Err(_) => {
                            let _ = events.send(WatchNote {
                                path,
                                signal: WatchSignal::Unavailable,
                            });
                            None
                        }
                    };
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let mut stop_watch = false;
        let mut failed: Option<PathBuf> = None;
        if let Some(current) = slot.as_mut() {
            match current.poll() {
                Ok(Some(signal)) => {
                    stop_watch = matches!(signal, WatchSignal::Unavailable);
                    let _ = events.send(WatchNote {
                        path: current.path().to_path_buf(),
                        signal,
                    });
                }
                Ok(None) => {}
                Err(_) => {
                    stop_watch = true;
                    failed = Some(current.path().to_path_buf());
                }
            }
        }
        if stop_watch {
            slot = None;
        }
        if let Some(path) = failed {
            let _ = events.send(WatchNote {
                path,
                signal: WatchSignal::Unavailable,
            });
        }
    }
}

fn signal_for_error(code: u32) -> WatchSignal {
    if code == ERROR_NOTIFY_ENUM_DIR.0 {
        WatchSignal::Overflow
    } else {
        WatchSignal::Unavailable
    }
}

/// Пустой или обрезанный буфер нельзя разбирать по именам: часть событий уже нет.
pub fn signal_for_buffer(buffer: &[u8], bytes: u32) -> WatchSignal {
    if bytes == 0 || parse_notify_records(buffer, bytes).is_err() {
        WatchSignal::Overflow
    } else {
        WatchSignal::Changed
    }
}

fn parse_notify_records(buffer: &[u8], bytes: u32) -> Result<(), ()> {
    let end = bytes as usize;
    if end == 0 || end > buffer.len() {
        return Err(());
    }
    let mut offset = 0usize;
    while offset + 12 <= end {
        let next = u32::from_le_bytes(buffer[offset..offset + 4].try_into().unwrap()) as usize;
        let name_len = u32::from_le_bytes(buffer[offset + 8..offset + 12].try_into().unwrap()) as usize;
        if offset + 12 + name_len > end {
            return Err(());
        }
        if next == 0 {
            return Ok(());
        }
        if next < 12 || offset + next > end {
            return Err(());
        }
        offset += next;
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_partial_record_handled() {
        let mut buffer = [0u8; 32];
        buffer[8..12].copy_from_slice(&40u32.to_le_bytes());
        assert_eq!(signal_for_buffer(&buffer, 16), WatchSignal::Overflow);
        assert_eq!(signal_for_buffer(&buffer, 0), WatchSignal::Overflow);
    }

    #[test]
    fn notify_complete_record_is_a_change_not_a_name_apply() {
        let mut buffer = [0u8; 32];
        buffer[8..12].copy_from_slice(&4u32.to_le_bytes());
        buffer[12..14].copy_from_slice(&u16::from(b'a').to_le_bytes());
        buffer[14..16].copy_from_slice(&u16::from(b'b').to_le_bytes());
        assert_eq!(signal_for_buffer(&buffer, 16), WatchSignal::Changed);
    }

    #[test]
    fn notify_handle_released_on_tab_close_or_dir_change() {
        let root = std::env::temp_dir().join(format!("fm-watch-handle-{}", std::process::id()));
        let first = root.join("a");
        let second = root.join("b");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let opened = DirWatchSlot::open(&first).unwrap();
        drop(opened);
        let retargeted = DirWatchSlot::open(&second).unwrap();
        assert_eq!(retargeted.path(), second.as_path());
        drop(retargeted);
        let _ = std::fs::remove_dir_all(root);
    }
}
