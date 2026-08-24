//! Centered in-shell QQ Music login surface.

use std::sync::Arc;

use gpui::{
    AnyElement, FontWeight, Image, ImageFormat, MouseButton, div, img, prelude::*, px, rgba, svg,
};
use gpui_effects::{FrostedGlass, FrostedGlassAppearance};
use hmp_desktop_common::UiLoginPhase;
use uic::assets::LucideIcons;

use crate::app::HmpGpuiApp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginAction {
    Start,
    Cancel,
    Logout,
}

pub const fn primary_action(phase: UiLoginPhase) -> LoginAction {
    match phase {
        UiLoginPhase::LoggedOut | UiLoginPhase::Expired | UiLoginPhase::Error => LoginAction::Start,
        UiLoginPhase::LoggedIn => LoginAction::Logout,
        UiLoginPhase::CreatingQr | UiLoginPhase::WaitingScan | UiLoginPhase::WaitingConfirm => {
            LoginAction::Cancel
        }
    }
}

fn default_status(phase: UiLoginPhase) -> &'static str {
    match phase {
        UiLoginPhase::LoggedOut => "获取二维码后使用 QQ 手机版扫码",
        UiLoginPhase::CreatingQr => "正在获取登录二维码…",
        UiLoginPhase::WaitingScan => "请用 QQ 手机版扫码",
        UiLoginPhase::WaitingConfirm => "已扫码，请在手机上确认",
        UiLoginPhase::Expired => "二维码已过期，请重新获取",
        UiLoginPhase::Error => "登录未完成，请重试",
        UiLoginPhase::LoggedIn => "QQ 音乐账号已连接",
    }
}

fn action_label(phase: UiLoginPhase) -> &'static str {
    match primary_action(phase) {
        LoginAction::Start if matches!(phase, UiLoginPhase::Expired | UiLoginPhase::Error) => {
            "重新获取二维码"
        }
        LoginAction::Start => "获取二维码",
        LoginAction::Cancel => "取消登录",
        LoginAction::Logout => "退出登录",
    }
}

fn qr_surface(app: &HmpGpuiApp) -> AnyElement {
    if app.events.auth.phase == UiLoginPhase::LoggedIn {
        return div()
            .size(px(214.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(18.))
            .bg(rgba(0xffffff0c))
            .child(
                svg()
                    .path(LucideIcons::CircleUserRound)
                    .size(px(92.))
                    .text_color(rgba(0xf44a6edc)),
            )
            .into_any_element();
    }

    if let Some(bytes) = &app.events.login_qr {
        let image = Arc::new(Image::from_bytes(ImageFormat::Png, bytes.clone()));
        return div()
            .size(px(214.))
            .p(px(9.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(18.))
            .bg(rgba(0xffffffff))
            .shadow(vec![
                gpui::BoxShadow::new(px(0.), px(8.), rgba(0x00000038).into()).blur_radius(px(24.)),
            ])
            .child(img(image).size_full())
            .into_any_element();
    }

    div()
        .size(px(214.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(18.))
        .border_1()
        .border_color(rgba(0xffffff13))
        .bg(rgba(0xffffff08))
        .text_size(px(12.))
        .text_color(rgba(0xe3e3e99a))
        .child("等待二维码")
        .into_any_element()
}

pub fn render(app: &mut HmpGpuiApp, cx: &mut gpui::Context<HmpGpuiApp>) -> impl IntoElement {
    let phase = app.events.auth.phase;
    let action = primary_action(phase);
    let status = if app.events.auth.message.is_empty() {
        default_status(phase).to_owned()
    } else {
        app.events.auth.message.clone()
    };
    let display_name = app.events.auth.display_name.clone();
    let should_cancel_on_close = phase != UiLoginPhase::LoggedIn;
    let glass = FrostedGlassAppearance::dark()
        .blur_radius(px(18.))
        .saturation(1.45)
        .brightness(1.04)
        .tint(rgba(0x17102fdc).into())
        .edge(rgba(0x7656d4a8).into())
        .edge_width(px(1.))
        .sheen(0.12);

    div()
        .id("qq-login-overlay")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x090a10ad))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            FrostedGlass::with_appearance(glass)
                .w(px(382.))
                .min_h(px(446.))
                .p(px(24.))
                .flex()
                .flex_col()
                .items_center()
                .rounded(px(25.))
                .border_1()
                .border_color(rgba(0x7656d48f))
                .shadow(vec![
                    gpui::BoxShadow::new(px(0.), px(18.), rgba(0x00000066).into())
                        .blur_radius(px(44.))
                        .spread_radius(px(-8.)),
                ])
                .child(
                    div()
                        .w_full()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(17.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgba(0xfff8f9f2))
                                .child("登录 QQ 音乐"),
                        )
                        .child(
                            div()
                                .id("qq-login-close")
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .hover(|style| style.bg(rgba(0xffffff14)))
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    if should_cancel_on_close {
                                        app.commands.cancel_login();
                                    }
                                    app.events.close_login_modal();
                                    cx.notify();
                                }))
                                .child(
                                    svg()
                                        .path(LucideIcons::X)
                                        .size(px(16.))
                                        .text_color(rgba(0xe9e9efc7)),
                                ),
                        ),
                )
                .child(div().h(px(16.)))
                .child(qr_surface(app))
                .child(div().h(px(18.)))
                .child(
                    div()
                        .h(px(40.))
                        .max_w(px(306.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_center()
                        .text_size(px(12.5))
                        .text_color(rgba(0xe3e3e9bd))
                        .child(status),
                )
                .when(phase == UiLoginPhase::LoggedIn, |panel| {
                    panel.child(
                        div()
                            .mt(px(2.))
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgba(0xfff8f9e8))
                            .child(display_name),
                    )
                })
                .child(div().flex_1().min_h(px(16.)))
                .child(
                    div()
                        .id("qq-login-primary")
                        .w_full()
                        .h(px(38.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(10.))
                        .cursor_pointer()
                        .bg(rgba(0xfa2d55e8))
                        .hover(|style| style.bg(rgba(0xff3e63f2)))
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgba(0xffffffff))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            match action {
                                LoginAction::Start => {
                                    app.events.open_login_modal();
                                    app.commands.start_login();
                                }
                                LoginAction::Cancel => {
                                    app.commands.cancel_login();
                                    app.events.close_login_modal();
                                }
                                LoginAction::Logout => app.commands.logout(),
                            }
                            cx.notify();
                        }))
                        .child(action_label(phase)),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_overlay_primary_action_tracks_auth_phase() {
        assert_eq!(primary_action(UiLoginPhase::LoggedOut), LoginAction::Start);
        assert_eq!(primary_action(UiLoginPhase::Expired), LoginAction::Start);
        assert_eq!(primary_action(UiLoginPhase::Error), LoginAction::Start);
        assert_eq!(
            primary_action(UiLoginPhase::WaitingScan),
            LoginAction::Cancel
        );
        assert_eq!(primary_action(UiLoginPhase::LoggedIn), LoginAction::Logout);
    }
}
