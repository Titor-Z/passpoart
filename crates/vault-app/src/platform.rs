//! 平台层：让系统自己画的东西让位给自绘标题栏（与 glowlink 同款做法）。
//!
//! 只 macOS 编译；Windows 侧 gpui-ce 已把整条非客户区去掉，没有按钮可藏。

use gpui::Window;

/// 藏掉系统标题栏按钮（关闭 / 最小化 / 缩放）。窗口创建回调里调用，幂等。
pub fn hide_system_window_buttons(window: &Window) {
    imp::hide(window)
}

#[cfg(target_os = "macos")]
mod imp {
    use gpui::Window;
    use objc2_app_kit::{NSWindowButton, NSView};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    pub fn hide(window: &Window) {
        // `Window` 的固有方法 `window_handle()` 会遮蔽 trait 方法，必须显式走 trait。
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        // SAFETY: 窗口创建回调在主线程；指针活到本函数结束。
        let view: &NSView = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        let Some(ns_window) = view.window() else {
            return;
        };
        for kind in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(button) = ns_window.standardWindowButton(kind) {
                button.setHidden(true);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use gpui::Window;
    pub fn hide(_window: &Window) {}
}
