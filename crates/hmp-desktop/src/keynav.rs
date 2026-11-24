//! 键盘导航（M7 焦点可达）的 Rust 侧接线。
//!
//! 焦点模型见 `ui/keynav.slint`：鼠标点击不改变键盘焦点，任意指针按下即
//! 退出键盘模式。Slint 语言层没有全局指针监听原语（事件被命中的 TouchArea
//! accept 后不再冒泡到祖先，i-slint-core input.rs send_mouse_event_to_item），
//! 指针活动由 winit 事件钩子（main.rs）登记到 `KeyNav.pointer-seq`，聚焦中
//! 的控件监听后 `clear-focus()`。无头/测试后端没有 winit 钩子，测试显式
//! 调用 [`pointer_activity`]。

use slint::Global;

use crate::{AppWindow, KeyNav};

/// 登记一次指针按下（UI 线程调用）：聚焦中的键盘控件随即让出焦点。
pub fn pointer_activity(ui: &AppWindow) {
    let keynav = KeyNav::get(ui);
    keynav.set_pointer_seq(keynav.get_pointer_seq() + 1);
}

/// 请求聚焦搜索框（全局 Ctrl+F）：`focus-search-seq` +1，SearchPage 监听后聚焦。
pub fn request_search_focus(ui: &AppWindow) {
    let keynav = KeyNav::get(ui);
    keynav.set_focus_search_seq(keynav.get_focus_search_seq() + 1);
}
