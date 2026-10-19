//! Windows non-client hit testing for the custom Slint title bar.
//! Winit 0.30.13's drag helpers post a pointer instead of a packed POINTS value;
//! return native caption/resize hit codes so Windows owns the entire move loop.
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::ScreenToClient,
    UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
};
const PREVIOUS_PROC_PROPERTY: &[u16] = &[
    87, 72, 83, 116, 117, 100, 105, 111, 67, 104, 114, 111, 109, 101, 80, 114, 111, 99, 0,
];
const TITLE_HEIGHT: i32 = 40;
const CONTROLS_WIDTH: i32 = 138;

pub(super) fn install(window: &slint::Window) {
    let handle = window.window_handle();
    let Ok(handle) = handle.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as HWND;
    install_icon(hwnd);
    unsafe {
        if !GetPropW(hwnd, PREVIOUS_PROC_PROPERTY.as_ptr()).is_null() {
            return;
        }
        let previous = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
        if previous == 0
            || SetPropW(
                hwnd,
                PREVIOUS_PROC_PROPERTY.as_ptr(),
                previous as *mut core::ffi::c_void,
            ) == 0
        {
            eprintln!("Could not enable native custom-titlebar hit testing");
            return;
        }
        if SetWindowLongPtrW(hwnd, GWLP_WNDPROC, subclass as *const () as isize) == 0 {
            RemovePropW(hwnd, PREVIOUS_PROC_PROPERTY.as_ptr());
            eprintln!("Could not attach native custom-titlebar hit testing");
        }
    }
}

pub(super) fn install_icon(hwnd: HWND) {
    unsafe {
        let module = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(core::ptr::null());
        // LR_SHARED keeps resource-backed icon handles valid for every application window.
        for (kind, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
            let size = GetSystemMetrics(metric);
            let icon = LoadImageW(
                module,
                101usize as *const u16,
                IMAGE_ICON,
                size,
                size,
                LR_SHARED,
            );
            if !icon.is_null() {
                SendMessageW(hwnd, WM_SETICON, kind as usize, icon as isize);
            }
        }
    }
}

/// Place secondary windows relative to their source window, never the desktop origin.
pub(super) fn show_centered<C: slint::ComponentHandle>(
    component: &C,
    parent: &slint::Window,
) -> Result<(), slint::PlatformError> {
    center_on_parent(component.window(), parent);
    component.show()?;
    install(component.window());
    center_on_parent(component.window(), parent);
    Ok(())
}

fn native_handle(window: &slint::Window) -> Option<HWND> {
    let handle = window.window_handle();
    let handle = handle.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}

pub(super) fn center_on_parent(child: &slint::Window, parent: &slint::Window) {
    let position = parent.position();
    let size = parent.size();
    let child_size = child.size();
    let mut owner = (
        position.x,
        position.y,
        size.width as i32,
        size.height as i32,
    );
    let mut work = None;
    if let Some(hwnd) = native_handle(parent) {
        unsafe {
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect) != 0 {
                owner = (
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                );
            }
            use windows_sys::Win32::Graphics::Gdi::{
                GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
            };
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: core::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(monitor, &mut info) != 0 {
                work = Some((
                    info.rcWork.left,
                    info.rcWork.top,
                    info.rcWork.right,
                    info.rcWork.bottom,
                ));
            }
        }
    }
    let (x, y) = centered_position(
        owner,
        (child_size.width as i32, child_size.height as i32),
        work,
    );
    child.set_position(slint::PhysicalPosition::new(x, y));
}

fn centered_position(
    parent: (i32, i32, i32, i32),
    child: (i32, i32),
    work: Option<(i32, i32, i32, i32)>,
) -> (i32, i32) {
    let mut x = parent.0 + (parent.2 - child.0) / 2;
    let mut y = parent.1 + (parent.3 - child.1) / 2;
    if let Some((left, top, right, bottom)) = work {
        x = x.clamp(left, (right - child.0).max(left));
        y = y.clamp(top, (bottom - child.1).max(top));
    }
    (x, y)
}

/// The WebView2 window lives on another event-loop thread; choose an app-owned
/// foreground window, or the exact main window, without changing authentication.
pub(super) fn center_native_on_app_window(child: HWND) {
    unsafe {
        let mut owner = GetAncestor(GetForegroundWindow(), GA_ROOT);
        let mut pid = 0;
        GetWindowThreadProcessId(owner, &mut pid);
        if owner == child || pid != std::process::id() {
            let title = "WallHaven Studio\0".encode_utf16().collect::<Vec<_>>();
            owner = FindWindowW(core::ptr::null(), title.as_ptr());
            GetWindowThreadProcessId(owner, &mut pid);
        }
        if owner.is_null() || owner == child || pid != std::process::id() {
            return;
        }
        let mut parent = RECT::default();
        let mut rect = RECT::default();
        if GetWindowRect(owner, &mut parent) == 0 || GetWindowRect(child, &mut rect) == 0 {
            return;
        }
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        };
        let mut monitor = MONITORINFO {
            cbSize: core::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if GetMonitorInfoW(
            MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        ) != 0
        {
            Some((
                monitor.rcWork.left,
                monitor.rcWork.top,
                monitor.rcWork.right,
                monitor.rcWork.bottom,
            ))
        } else {
            None
        };
        let (x, y) = centered_position(
            (
                parent.left,
                parent.top,
                parent.right - parent.left,
                parent.bottom - parent.top,
            ),
            (rect.right - rect.left, rect.bottom - rect.top),
            work,
        );
        SetWindowPos(
            child,
            core::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn hit_test(x: i32, y: i32, width: i32, height: i32, dpi: u32, maximized: bool) -> i32 {
    if x < 0 || y < 0 || x >= width || y >= height {
        return HTNOWHERE as i32;
    }
    let scale = |n: i32| ((n as i64 * dpi.max(96) as i64 + 48) / 96) as i32;
    let border = scale(6);
    if !maximized {
        let left = x < border;
        let right = x >= width - border;
        let top = y < border;
        let bottom = y >= height - border;
        match (left, right, top, bottom) {
            (true, _, true, _) => return HTTOPLEFT as i32,
            (_, true, true, _) => return HTTOPRIGHT as i32,
            (true, _, _, true) => return HTBOTTOMLEFT as i32,
            (_, true, _, true) => return HTBOTTOMRIGHT as i32,
            (true, _, _, _) => return HTLEFT as i32,
            (_, true, _, _) => return HTRIGHT as i32,
            (_, _, true, _) => return HTTOP as i32,
            (_, _, _, true) => return HTBOTTOM as i32,
            _ => {}
        }
    }
    if y < scale(TITLE_HEIGHT) && x < width - scale(CONTROLS_WIDTH) {
        HTCAPTION as i32
    } else {
        HTCLIENT as i32
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let previous = unsafe { GetPropW(hwnd, PREVIOUS_PROC_PROPERTY.as_ptr()) } as isize;
    if message == WM_NCHITTEST {
        let mut point = POINT {
            x: (lparam as u16 as i16) as i32,
            y: ((lparam as usize >> 16) as u16 as i16) as i32,
        };
        let mut rect = RECT::default();
        if unsafe { ScreenToClient(hwnd, &mut point) } != 0
            && unsafe { GetClientRect(hwnd, &mut rect) } != 0
        {
            return hit_test(
                point.x,
                point.y,
                rect.right,
                rect.bottom,
                unsafe { GetDpiForWindow(hwnd) },
                unsafe { IsZoomed(hwnd) } != 0,
            ) as LRESULT;
        }
    }
    if message == WM_NCDESTROY {
        unsafe {
            if previous != 0
                && GetWindowLongPtrW(hwnd, GWLP_WNDPROC) == subclass as *const () as isize
            {
                SetWindowLongPtrW(hwnd, GWLP_WNDPROC, previous);
            }
            RemovePropW(hwnd, PREVIOUS_PROC_PROPERTY.as_ptr());
        }
    }
    if previous == 0 {
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    } else {
        unsafe {
            CallWindowProcW(
                Some(core::mem::transmute::<
                    isize,
                    unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
                >(previous)),
                hwnd,
                message,
                wparam,
                lparam,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secondary_position_centers_on_owner_and_clamps_to_work_area() {
        assert_eq!(
            centered_position((400, 200, 1280, 820), (640, 600), None),
            (720, 310)
        );
        assert_eq!(
            centered_position(
                (-1800, 100, 1280, 820),
                (800, 600),
                Some((-1920, 0, 0, 1080))
            ),
            (-1560, 210)
        );
        assert_eq!(
            centered_position((1700, 950, 400, 300), (800, 600), Some((0, 0, 1920, 1080))),
            (1120, 480)
        );
    }
    #[test]
    fn caption_controls_and_resize_edges_do_not_overlap() {
        assert_eq!(hit_test(400, 20, 1280, 820, 96, false), HTCAPTION as i32);
        assert_eq!(hit_test(1150, 20, 1280, 820, 96, false), HTCLIENT as i32);
        assert_eq!(hit_test(1210, 20, 1280, 820, 96, false), HTCLIENT as i32);
        assert_eq!(hit_test(1255, 20, 1280, 820, 96, false), HTCLIENT as i32);
        assert_eq!(hit_test(400, 60, 1280, 820, 96, false), HTCLIENT as i32);
        assert_eq!(
            hit_test(1278, 818, 1280, 820, 96, false),
            HTBOTTOMRIGHT as i32
        );
        assert_eq!(hit_test(2, 2, 1280, 820, 96, false), HTTOPLEFT as i32);
        assert_eq!(hit_test(1278, 818, 1280, 820, 96, true), HTCLIENT as i32);
        assert_eq!(hit_test(600, 30, 1920, 1230, 144, false), HTCAPTION as i32);
        assert_eq!(hit_test(1740, 30, 1920, 1230, 144, false), HTCLIENT as i32);
        assert_eq!(hit_test(8, 300, 1920, 1230, 144, false), HTLEFT as i32);
        assert_eq!(hit_test(-1, 20, 1280, 820, 96, false), HTNOWHERE as i32);
    }
}
