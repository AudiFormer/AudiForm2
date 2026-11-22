// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The Audiform2 contributors.

//! Plug-in windows (Windows): a top-level window owned by the app's main
//! window, the plug-in's view in a child window, and a strip along the
//! bottom with a power button.

/// Whether a window on a screen at `dpi` is made DPI aware. At whole 25%
/// steps (100, 125, 150 ...) it is not: Windows stretches it, so a plug-in
/// that does not scale itself still shows whole. In between (115%) it is,
/// and the plug-in is told the scale, since a stretched window there drew
/// some plug-ins small in a corner. `AF2_PLUGIN_DPI=aware` or `=stretch`
/// overrides this, for testing.
pub fn scale_aware(dpi: u32, override_: Option<&str>) -> bool {
    match override_.map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("aware") => return true,
        Some(v) if v.eq_ignore_ascii_case("stretch") => return false,
        _ => {}
    }
    let percent = (dpi * 100 + 48) / 96;
    !percent.is_multiple_of(25)
}

#[cfg(windows)]
pub use win::{pump_messages, Window};

/// The VST 3 key code (`VirtualKeyCodes`) of a Windows virtual key; 0 for
/// a key that is a plain character.
pub fn vst_key_code(vk: u16) -> i16 {
    use vst3::Steinberg::VirtualKeyCodes_::*;
    let c = match vk {
        0x08 => KEY_BACK,
        0x09 => KEY_TAB,
        0x0C => KEY_CLEAR,
        0x0D => KEY_RETURN,
        0x13 => KEY_PAUSE,
        0x1B => KEY_ESCAPE,
        0x20 => KEY_SPACE,
        0x21 => KEY_PAGEUP,
        0x22 => KEY_PAGEDOWN,
        0x23 => KEY_END,
        0x24 => KEY_HOME,
        0x25 => KEY_LEFT,
        0x26 => KEY_UP,
        0x27 => KEY_RIGHT,
        0x28 => KEY_DOWN,
        0x29 => KEY_SELECT,
        0x2A => KEY_PRINT,
        0x2C => KEY_SNAPSHOT,
        0x2D => KEY_INSERT,
        0x2E => KEY_DELETE,
        0x2F => KEY_HELP,
        0x60..=0x69 => KEY_NUMPAD0 + (vk - 0x60) as i32,
        0x6A => KEY_MULTIPLY,
        0x6B => KEY_ADD,
        0x6C => KEY_SEPARATOR,
        0x6D => KEY_SUBTRACT,
        0x6E => KEY_DECIMAL,
        0x6F => KEY_DIVIDE,
        0x70..=0x7B => KEY_F1 + (vk - 0x70) as i32,
        0x90 => KEY_NUMLOCK,
        0x91 => KEY_SCROLL,
        0x10 => KEY_SHIFT,
        0x11 => KEY_CONTROL,
        0x12 => KEY_ALT,
        _ => 0,
    };
    c as i16
}

#[cfg(not(windows))]
pub struct Window;

#[cfg(not(windows))]
pub fn pump_messages() {}

#[cfg(windows)]
mod win {
    use std::cell::Cell;

    use vst3::Steinberg::Vst::{IEditController, IEditControllerTrait};
    use vst3::Steinberg::{
        kResultOk, kResultTrue, tresult, IPlugFrame, IPlugFrameTrait, IPlugView,
        IPlugViewContentScaleSupport, IPlugViewContentScaleSupportTrait, IPlugViewTrait, ViewRect,
    };
    use vst3::{Class, ComPtr, ComWrapper};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{
        GetDpiForMonitor, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        DPI_AWARENESS_CONTEXT_UNAWARE, MDT_EFFECTIVE_DPI,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use crate::host::com::{Edit, Edits};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    const TOP_CLASS: &str = "AudiForm2PluginWindow";
    const VIEW_CLASS: &str = "AudiForm2PluginView";

    /// Strip height at 96 DPI.
    const STRIP: i32 = 22;

    struct State {
        view: ComPtr<IPlugView>,
        child: HWND,
        strip: i32,
        bypassed: Cell<bool>,
        resizable: bool,
        edits: Edits,
        /// Set while the window follows the plug-in's own resize, so the
        /// size change is not handed back to it.
        following: Cell<bool>,
        /// The plug-in's window that had the keyboard when ours was last
        /// switched away from (a value box being typed in), given it back.
        last_focus: Cell<HWND>,
    }

    /// The plug-in's frame: it asks here to be resized.
    struct Frame {
        top: Cell<HWND>,
        strip: Cell<i32>,
        state: Cell<*const State>,
    }

    impl Class for Frame {
        type Interfaces = (IPlugFrame,);
    }

    impl IPlugFrameTrait for Frame {
        unsafe fn resizeView(&self, view: *mut IPlugView, size: *mut ViewRect) -> tresult {
            let top = self.top.get();
            if top.is_null() || size.is_null() {
                return kResultTrue;
            }
            let r = *size;
            let st = self.state.get();
            if !st.is_null() {
                (*st).following.set(true);
            }
            fit_top(top, r.right - r.left, r.bottom - r.top + self.strip.get());
            if !view.is_null() {
                let mut r = r;
                ((*(*view).vtbl).onSize)(view, &mut r);
            }
            if !st.is_null() {
                (*st).following.set(false);
                fit_child(top, &*st);
            }
            kResultTrue
        }
    }

    unsafe fn fit_top(top: HWND, w: i32, h: i32) {
        let style = GetWindowLongW(top, GWL_STYLE) as u32;
        let ex = GetWindowLongW(top, GWL_EXSTYLE) as u32;
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: w.max(100),
            bottom: h.max(60),
        };
        AdjustWindowRectEx(&mut rc, style, 0, ex);
        SetWindowPos(
            top,
            std::ptr::null_mut(),
            0,
            0,
            rc.right - rc.left,
            rc.bottom - rc.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }

    unsafe fn fit_child(top: HWND, st: &State) {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(top, &mut rc);
        let w = rc.right - rc.left;
        let h = (rc.bottom - rc.top - st.strip).max(1);
        MoveWindow(st.child, 0, 0, w, h, 1);
        InvalidateRect(top, std::ptr::null(), 0);
    }

    fn register() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let top = wide(TOP_CLASS);
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(top_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: top.as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            RegisterClassExW(&wc);
            let view = wide(VIEW_CLASS);
            let wc = WNDCLASSEXW {
                lpfnWndProc: Some(view_proc),
                lpszClassName: view.as_ptr(),
                style: 0,
                ..wc
            };
            RegisterClassExW(&wc);
        });
    }

    unsafe fn state_of(hwnd: HWND) -> Option<&'static State> {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State;
        p.as_ref()
    }

    /// The plug-in's own window inside ours (the view's first child), or
    /// the view itself before the plug-in made one.
    unsafe fn plugin_hwnd(st: &State) -> HWND {
        let c = GetWindow(st.child, GW_CHILD);
        if c.is_null() {
            st.child
        } else {
            c
        }
    }

    /// Puts the keyboard in the plug-in's window: where it last was (a
    /// value box being typed in), else the plug-in's main window. Without
    /// this the keyboard stays on our frame window, and a value box the
    /// plug-in opens on a double-click never hears the keys.
    unsafe fn give_focus(top: HWND, st: &State) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
        let now = GetFocus();
        if !now.is_null() && now != top && now != st.child && IsChild(top, now) != 0 {
            return;
        }
        let last = st.last_focus.get();
        let to = if !last.is_null() && IsWindow(last) != 0 && IsChild(top, last) != 0 {
            last
        } else {
            plugin_hwnd(st)
        };
        SetFocus(to);
    }

    unsafe extern "system" fn view_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            // The keyboard goes on to the plug-in's window.
            WM_SETFOCUS => {
                let c = GetWindow(hwnd, GW_CHILD);
                if !c.is_null() {
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(c);
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    unsafe extern "system" fn top_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_ACTIVATE => {
                if let Some(st) = state_of(hwnd) {
                    if (wp & 0xffff) as u32 == WA_INACTIVE {
                        let f = windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus();
                        if !f.is_null() && IsChild(hwnd, f) != 0 {
                            st.last_focus.set(f);
                        }
                    } else if IsIconic(hwnd) == 0 {
                        give_focus(hwnd, st);
                        return 0;
                    }
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_SETFOCUS => {
                if let Some(st) = state_of(hwnd) {
                    give_focus(hwnd, st);
                }
                0
            }
            WM_CLOSE => {
                if let Some(st) = state_of(hwnd) {
                    if let Ok(mut e) = st.edits.lock() {
                        e.push(Edit::EditorClosed);
                    }
                }
                // The host drops the window, which destroys it.
                ShowWindow(hwnd, SW_HIDE);
                0
            }
            WM_ERASEBKGND => 1,
            WM_SIZE => {
                if let Some(st) = state_of(hwnd) {
                    fit_child(hwnd, st);
                    if st.resizable && !st.following.get() {
                        let mut rc: RECT = std::mem::zeroed();
                        GetClientRect(st.child, &mut rc);
                        let mut r = ViewRect {
                            left: 0,
                            top: 0,
                            right: rc.right,
                            bottom: rc.bottom,
                        };
                        st.view.onSize(&mut r);
                    }
                }
                0
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                if let Some(st) = state_of(hwnd) {
                    paint_strip(hwnd, dc, st);
                }
                EndPaint(hwnd, &ps);
                0
            }
            WM_LBUTTONDOWN => {
                if let Some(st) = state_of(hwnd) {
                    let x = (lp & 0xffff) as i16 as i32;
                    let y = ((lp >> 16) & 0xffff) as i16 as i32;
                    let mut rc: RECT = std::mem::zeroed();
                    GetClientRect(hwnd, &mut rc);
                    if y >= rc.bottom - st.strip && x < st.strip * 5 {
                        let b = !st.bypassed.get();
                        st.bypassed.set(b);
                        if let Ok(mut e) = st.edits.lock() {
                            e.push(Edit::Bypass(b));
                        }
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    unsafe fn paint_strip(hwnd: HWND, dc: HDC, st: &State) {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let strip = RECT {
            left: 0,
            top: rc.bottom - st.strip,
            right: rc.right,
            bottom: rc.bottom,
        };
        let bg = CreateSolidBrush(0x0020_1a18);
        FillRect(dc, &strip, bg);
        DeleteObject(bg);
        let on = !st.bypassed.get();
        let color = if on { 0x00f0_c040 } else { 0x0080_8080 };
        let pen = CreatePen(PS_SOLID, (st.strip / 10).max(1), color);
        let old_pen = SelectObject(dc, pen);
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        let m = st.strip / 4;
        Ellipse(
            dc,
            m * 2,
            strip.top + m,
            m * 2 + st.strip - m * 2,
            strip.bottom - m,
        );
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        DeleteObject(pen);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, color);
        let mut label = RECT {
            left: st.strip + m * 2,
            ..strip
        };
        let mut text = wide(if on { "On" } else { "Bypassed" });
        DrawTextW(
            dc,
            text.as_mut_ptr(),
            -1,
            &mut label,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
        SetTextColor(dc, 0x00a0_a0a0);
        let mut right = RECT {
            right: strip.right - m * 2,
            ..strip
        };
        let mut brand = wide("AudiForm 2");
        DrawTextW(
            dc,
            brand.as_mut_ptr(),
            -1,
            &mut right,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE,
        );
    }

    /// The DPI of the screen the window will open on.
    unsafe fn monitor_dpi(owner: HWND) -> u32 {
        // Ask while DPI aware, or Windows answers 96 for everyone.
        let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mon = if owner.is_null() {
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST)
        } else {
            MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST)
        };
        let (mut x, mut y) = (96u32, 96u32);
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut x, &mut y) != 0 {
            x = 96;
        }
        SetThreadDpiAwarenessContext(old);
        x
    }

    /// An open plug-in window. Dropping it closes it.
    pub struct Window {
        top: HWND,
        state: *mut State,
        _frame: ComWrapper<Frame>,
    }

    impl Window {
        pub fn open(
            controller: &ComPtr<IEditController>,
            title: &str,
            owner: u64,
            bypassed: bool,
            edits: Edits,
        ) -> Result<Window, String> {
            unsafe { Self::open_inner(controller, title, owner, bypassed, edits) }
        }

        unsafe fn open_inner(
            controller: &ComPtr<IEditController>,
            title: &str,
            owner: u64,
            bypassed: bool,
            edits: Edits,
        ) -> Result<Window, String> {
            let raw = controller.createView(c"editor".as_ptr());
            let view = ComPtr::<IPlugView>::from_raw(raw)
                .ok_or("This plug-in has no window of its own.")?;
            if view.isPlatformTypeSupported(c"HWND".as_ptr()) != kResultTrue {
                return Err("This plug-in's window does not work on Windows.".into());
            }
            register();
            let owner = owner as HWND;
            let dpi = monitor_dpi(owner);
            let aware = super::scale_aware(dpi, std::env::var("AF2_PLUGIN_DPI").ok().as_deref());
            let old = SetThreadDpiAwarenessContext(if aware {
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
            } else {
                DPI_AWARENESS_CONTEXT_UNAWARE
            });
            let scale = if aware { dpi as f32 / 96.0 } else { 1.0 };
            if aware {
                if let Some(s) = view.cast::<IPlugViewContentScaleSupport>() {
                    s.setContentScaleFactor(scale);
                }
            }
            let strip = (STRIP as f32 * scale).round() as i32;
            let resizable = view.canResize() == kResultTrue;
            let mut size = ViewRect {
                left: 0,
                top: 0,
                right: 400,
                bottom: 300,
            };
            view.getSize(&mut size);
            let w = (size.right - size.left).max(100);
            let h = (size.bottom - size.top).max(60);
            let mut style =
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
            if resizable {
                style |= WS_THICKFRAME | WS_MAXIMIZEBOX;
            }
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h + strip,
            };
            AdjustWindowRectEx(&mut rc, style, 0, 0);
            let inst = GetModuleHandleW(std::ptr::null());
            let class = wide(TOP_CLASS);
            let name = wide(title);
            let top = CreateWindowExW(
                0,
                class.as_ptr(),
                name.as_ptr(),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rc.right - rc.left,
                rc.bottom - rc.top,
                owner,
                std::ptr::null_mut(),
                inst,
                std::ptr::null(),
            );
            if top.is_null() {
                SetThreadDpiAwarenessContext(old);
                return Err("The window could not be made.".into());
            }
            let vclass = wide(VIEW_CLASS);
            let child = CreateWindowExW(
                0,
                vclass.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                w,
                h,
                top,
                std::ptr::null_mut(),
                inst,
                std::ptr::null(),
            );
            SetThreadDpiAwarenessContext(old);
            let state = Box::into_raw(Box::new(State {
                view: view.clone(),
                child,
                strip,
                bypassed: Cell::new(bypassed),
                resizable,
                edits,
                following: Cell::new(false),
                last_focus: Cell::new(std::ptr::null_mut()),
            }));
            SetWindowLongPtrW(top, GWLP_USERDATA, state as isize);
            let frame = ComWrapper::new(Frame {
                top: Cell::new(top),
                strip: Cell::new(strip),
                state: Cell::new(state),
            });
            if let Some(f) = frame.to_com_ptr::<IPlugFrame>() {
                view.setFrame(f.as_ptr());
            }
            if view.attached(child, c"HWND".as_ptr()) != kResultOk {
                view.setFrame(std::ptr::null_mut());
                SetWindowLongPtrW(top, GWLP_USERDATA, 0);
                DestroyWindow(top);
                drop(Box::from_raw(state));
                return Err("The plug-in would not open its window.".into());
            }
            ShowWindow(top, SW_SHOW);
            SetForegroundWindow(top);
            give_focus(top, &*state);
            Ok(Window {
                top,
                state,
                _frame: frame,
            })
        }

        pub fn set_bypassed(&self, b: bool) {
            unsafe {
                (*self.state).bypassed.set(b);
                InvalidateRect(self.top, std::ptr::null(), 0);
            }
        }

        pub fn hide(&self) {
            unsafe {
                ShowWindow(self.top, SW_HIDE);
            }
        }

        /// A new title (the track or slot may have been renamed since).
        pub fn set_title(&self, title: &str) {
            let t = wide(title);
            unsafe {
                SetWindowTextW(self.top, t.as_ptr());
            }
        }

        pub fn show(&self) {
            unsafe {
                if IsIconic(self.top) != 0 {
                    ShowWindow(self.top, SW_RESTORE);
                }
                ShowWindow(self.top, SW_SHOW);
                SetForegroundWindow(self.top);
                give_focus(self.top, &*self.state);
            }
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let st = &*self.state;
                st.view.setFrame(std::ptr::null_mut());
                st.view.removed();
                SetWindowLongPtrW(self.top, GWLP_USERDATA, 0);
                DestroyWindow(self.top);
                drop(Box::from_raw(self.state));
            }
        }
    }

    /// Runs this thread's window messages. Called on every pass of the
    /// host's main loop while any plug-in is loaded, window or not: many
    /// plug-ins finish loading through timers and window messages.
    pub fn pump_messages() {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if space_for_app(&msg) {
                    continue;
                }
                if keys_to_view(&msg) {
                    continue;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    /// Keys that land on our own frame or view window (the plug-in has no
    /// window of its own that takes the keyboard) go to the plug-in
    /// through `IPlugView::onKeyDown` / `onKeyUp`, as the VST 3 standard
    /// has it. True when the plug-in took the key.
    unsafe fn keys_to_view(msg: &MSG) -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            GetKeyState, MapVirtualKeyW, MAPVK_VK_TO_CHAR, VK_CONTROL, VK_MENU, VK_SHIFT,
        };
        let down = matches!(msg.message, WM_KEYDOWN | WM_SYSKEYDOWN);
        let up = matches!(msg.message, WM_KEYUP | WM_SYSKEYUP);
        if !down && !up {
            return false;
        }
        let root = GetAncestor(msg.hwnd, GA_ROOT);
        let Some(st) = (if root.is_null() { None } else { state_of(root) }) else {
            return false;
        };
        // The plug-in's own windows hear their keys as usual.
        if msg.hwnd != root && msg.hwnd != st.child {
            return false;
        }
        let vk = msg.wParam as u16;
        let code = super::vst_key_code(vk);
        let ch = if code == 0 {
            MapVirtualKeyW(vk as u32, MAPVK_VK_TO_CHAR) & 0xffff
        } else {
            0
        };
        let mut key = char::from_u32(ch).unwrap_or('\0');
        if GetKeyState(VK_SHIFT as i32) >= 0 {
            key = key.to_ascii_lowercase();
        }
        if key == '\0' && code == 0 {
            return false;
        }
        let mut mods = 0i16;
        if GetKeyState(VK_SHIFT as i32) < 0 {
            mods |= 1;
        }
        if GetKeyState(VK_MENU as i32) < 0 {
            mods |= 2;
        }
        if GetKeyState(VK_CONTROL as i32) < 0 {
            // VST 3: kCommandKey is Ctrl on Windows.
            mods |= 4;
        }
        let k = key as u32 as u16;
        let r = if down {
            st.view.onKeyDown(k, code, mods)
        } else {
            st.view.onKeyUp(k, code, mods)
        };
        r == kResultTrue
    }

    /// Space (no Ctrl or Alt) in a plug-in window, outside a text box,
    /// plays or stops the app instead: reported, and kept from the plug-in.
    unsafe fn space_for_app(msg: &MSG) -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT, VK_SPACE,
        };
        let key = matches!(msg.message, WM_KEYDOWN | WM_KEYUP) && msg.wParam == VK_SPACE as usize;
        let ch = msg.message == WM_CHAR && msg.wParam == ' ' as usize;
        if !key && !ch {
            return false;
        }
        if GetKeyState(VK_CONTROL as i32) < 0 || GetKeyState(VK_MENU as i32) < 0 {
            return false;
        }
        let root = GetAncestor(msg.hwnd, GA_ROOT);
        let Some(st) = (if root.is_null() { None } else { state_of(root) }) else {
            return false;
        };
        let mut class = [0u16; 64];
        let n = GetClassNameW(msg.hwnd, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
        let name = String::from_utf16_lossy(&class[..n]).to_ascii_lowercase();
        if name.contains("edit") || name.contains("text") {
            return false;
        }
        if msg.message == WM_KEYDOWN && (msg.lParam >> 30) & 1 == 0 {
            if let Ok(mut e) = st.edits.lock() {
                e.push(Edit::Space(GetKeyState(VK_SHIFT as i32) < 0));
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dpi_rule_stretches_at_whole_steps_only() {
        for dpi in [96, 120, 144, 168, 192] {
            assert!(!super::scale_aware(dpi, None), "{dpi}");
        }
        for dpi in [110, 106, 130] {
            assert!(super::scale_aware(dpi, None), "{dpi}");
        }
        assert!(super::scale_aware(96, Some("aware")));
        assert!(!super::scale_aware(110, Some(" Stretch ")));
    }

    #[test]
    fn key_codes_follow_the_vst3_table() {
        use vst3::Steinberg::VirtualKeyCodes_::*;
        assert_eq!(super::vst_key_code(0x08), KEY_BACK as i16);
        assert_eq!(super::vst_key_code(0x0D), KEY_RETURN as i16);
        assert_eq!(super::vst_key_code(0x21), KEY_PAGEUP as i16);
        assert_eq!(super::vst_key_code(0x2E), KEY_DELETE as i16);
        assert_eq!(super::vst_key_code(0x65), KEY_NUMPAD5 as i16);
        assert_eq!(super::vst_key_code(0x7B), KEY_F12 as i16);
        assert_eq!(super::vst_key_code(0x12), KEY_ALT as i16);
        // Letters and digits are characters, not codes.
        assert_eq!(super::vst_key_code(b'A' as u16), 0);
        assert_eq!(super::vst_key_code(b'7' as u16), 0);
    }
}
