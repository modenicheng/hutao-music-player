//! 系统托盘（spec §4.2；feature `tray`）。
//!
//! 平台分派：Windows 走 tray-icon（Win32 原生），Unix 走 ksni
//! （D-Bus StatusNotifierItem）。两条路径共用同一契约：
//! - 输入：命令经 `Request` 通道，与 CLI/MPRIS/SMTC 同源；
//! - 输出：订阅 `DaemonState` 仅用于图标/菜单标签/tooltip 切换；
//! - 生命周期：`spawn_tray` 返回 RAII 守卫 [`Tray`]，Drop 时优雅关停；
//!   无桌面会话或创建失败返回 `None`（warn 不 panic，播放不受影响）。
//!
//! 守卫由 `serve.rs` 持有到进程收尾，覆盖所有退出路径（含错误提前返回）。

#[cfg(all(unix, feature = "tray"))]
mod ksni_tray;
#[cfg(all(unix, feature = "tray"))]
pub use ksni_tray::{Tray, spawn_tray};

#[cfg(all(windows, feature = "tray"))]
mod win_tray;
#[cfg(all(windows, feature = "tray"))]
pub use win_tray::{Tray, spawn_tray};
