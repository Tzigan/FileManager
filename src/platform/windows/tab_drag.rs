use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetCapture, ReleaseCapture, SetCapture, VK_ESCAPE, VK_LBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetCursorPos, HWND_TOPMOST, IsWindow, SWP_NOACTIVATE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SetWindowPos,
};
use windows::core::PCWSTR;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TabDragPreviewWindow(HWND);

impl TabDragPreviewWindow {
    pub(crate) fn find(title: &str) -> Option<Self> {
        let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let window = unsafe { FindWindowW(None, PCWSTR(title.as_ptr())) }.ok()?;
        (window != HWND::default()).then_some(Self(window))
    }

    pub(crate) fn is_valid(&self) -> bool {
        unsafe { IsWindow(Some(self.0)).as_bool() }
    }

    pub(crate) fn set_position(&self, x: i32, y: i32) -> bool {
        unsafe {
            SetWindowPos(
                self.0,
                Some(HWND_TOPMOST),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .is_ok()
        }
    }

    pub(crate) fn capture_mouse(&self) -> bool {
        unsafe {
            SetCapture(self.0);
            GetCapture() == self.0
        }
    }

    pub(crate) fn owns_mouse_capture(&self) -> bool {
        unsafe { GetCapture() == self.0 }
    }

    pub(crate) fn release_mouse_capture(&self) {
        if self.owns_mouse_capture() {
            let _ = unsafe { ReleaseCapture() };
        }
    }
}

pub(crate) fn cursor_screen_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    unsafe {
        GetCursorPos(&mut point).ok()?;
    }
    Some((point.x, point.y))
}

pub(crate) fn left_mouse_button_down() -> bool {
    (unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) }) < 0
}

pub(crate) fn escape_key_down() -> bool {
    (unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) }) < 0
}
