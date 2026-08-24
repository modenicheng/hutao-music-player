//! GPUI application view. Business state remains outside this module.

use std::time::Instant;

use gpui::{AppContext, Render, Subscription, Window, div, prelude::*, px, rgb, rgba};
use hmp_core::PlaybackState;
use uic::components::input::{InputEvent, TextInput};

use crate::{
    bridge::{CoreBridge, CoreCommandSender},
    components::{content, login_overlay, lyrics_panel, now_playing, player_bar, sidebar, top_bar},
    state::{EventState, NavigationState, Page},
    theme::{BACKGROUND, layout},
};

pub struct EffectsViewer {
    pub cover_key: String,
    pub started_at: Instant,
}

pub struct HmpGpuiApp {
    pub navigation: NavigationState,
    pub playback: PlaybackState,
    pub events: EventState,
    pub commands: CoreCommandSender,
    pub search_input: gpui::Entity<TextInput>,
    pub now_playing: bool,
    pub now_playing_lyrics: bool,
    pub effects_viewer: Option<EffectsViewer>,
    pub lyrics_panel: lyrics_panel::LyricsPanelState,
    _core_bridge: CoreBridge,
    _subscriptions: Vec<Subscription>,
}

impl HmpGpuiApp {
    pub fn new(
        mut core_bridge: CoreBridge,
        cx: &mut gpui::Context<Self>,
        window: &mut Window,
    ) -> Self {
        let search_input = cx.new(|cx| TextInput::new(cx).placeholder("搜索音乐"));
        let search_subscription = cx.subscribe(&search_input, |app, _, event: &InputEvent, cx| {
            app.navigation.navigate(Page::Search);
            if let InputEvent::Submit(text) = event {
                let query = text.trim();
                if !query.is_empty() {
                    app.events.begin_search();
                    app.commands.search(query.to_owned());
                }
            }
            cx.notify();
        });
        cx.focus_view(&search_input, window);

        let commands = core_bridge.commands();
        let mut playback_rx = core_bridge.playback_receiver();
        let playback = playback_rx.borrow().clone();
        cx.spawn(async move |this, cx| {
            while playback_rx.changed().await.is_ok() {
                let playback = playback_rx.borrow().clone();
                let Some(this) = this.upgrade() else {
                    break;
                };
                this.update(cx, |app, cx| {
                    app.playback = playback;
                    cx.notify();
                });
            }
        })
        .detach();

        let mut event_rx = core_bridge.take_event_receiver();
        cx.spawn(async move |this, cx| {
            while let Some(event) = event_rx.recv().await {
                let Some(this) = this.upgrade() else {
                    break;
                };
                this.update(cx, |app, cx| {
                    app.events.apply(event);
                    cx.notify();
                });
            }
        })
        .detach();

        Self {
            navigation: NavigationState::default(),
            playback,
            events: EventState::default(),
            commands,
            search_input,
            now_playing: false,
            now_playing_lyrics: true,
            effects_viewer: None,
            lyrics_panel: lyrics_panel::LyricsPanelState::default(),
            _core_bridge: core_bridge,
            _subscriptions: vec![search_subscription],
        }
    }
}

impl Render for HmpGpuiApp {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        window.set_traffic_light_position(if self.now_playing {
            gpui::point(px(17.), px(28.))
        } else {
            gpui::point(px(34.), px(34.))
        });

        let width = window.bounds().size.width;
        let height = window.bounds().size.height;
        if self.now_playing {
            // AlbumGlow is time-based; ordinary pages remain completely event-driven.
            window.request_animation_frame();
            return now_playing::render(self, width.as_f32(), height.as_f32(), cx);
        }

        let regular = layout::regular_layout(width.as_f32());
        let compact = regular.compact;
        let hide_lyrics = regular.hide_lyrics;
        let lyrics_page = self.navigation.page == Page::Lyrics;
        let main_content = if lyrics_page {
            lyrics_panel::render_fullscreen(&self.playback, &self.events, &mut self.lyrics_panel)
        } else {
            content::render(self, cx)
        };
        let inline_lyrics = (!hide_lyrics && !lyrics_page).then(|| {
            lyrics_panel::render_inline(&self.playback, &self.events, &mut self.lyrics_panel)
        });

        div()
            .size_full()
            .relative()
            .when(cfg!(target_os = "linux"), |root| {
                root.font_family(crate::theme::FONT_FAMILY)
            })
            .flex()
            .overflow_hidden()
            .when(cfg!(not(target_os = "windows")), |root| {
                root.rounded(px(22.))
            })
            .bg(rgb(BACKGROUND))
            .border_1()
            .border_color(rgba(0x00000045))
            .shadow(vec![
                gpui::BoxShadow::new(px(0.), px(11.), rgba(0x00000042).into())
                    .blur_radius(px(28.))
                    .spread_radius(px(-7.)),
            ])
            .child(
                div()
                    .m_5()
                    .flex()
                    .flex_col()
                    .child(sidebar::render(self, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(rgb(0x20222d))
                            .child(top_bar::render(self, compact))
                            .child(main_content)
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .bottom(px(layout::FLOATING_PLAYER_BOTTOM))
                                    .flex()
                                    .justify_center()
                                    .child(player_bar::render(self, cx, compact)),
                            ),
                    )
                    .when_some(inline_lyrics, |body, panel| body.child(panel)),
            )
            .when(self.events.login_modal_open, |root| {
                root.child(login_overlay::render(self, cx))
            })
            .into_any_element()
    }
}
