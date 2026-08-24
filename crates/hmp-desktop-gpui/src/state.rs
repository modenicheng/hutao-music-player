use hmp_desktop_common::{
    AppEvent, UiAuthData, UiLoginPhase, UiLyricData, UiPlaylistData, UiPlaylistTrackData,
    UiQueueData, UiSongData,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Library,
    Recommend,
    Search,
    Queue,
    Lyrics,
    Settings,
}

impl Page {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Library => "媒体库",
            Self::Recommend => "推荐",
            Self::Search => "搜索",
            Self::Queue => "播放队列",
            Self::Lyrics => "歌词",
            Self::Settings => "设置",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NavigationState {
    pub page: Page,
    pub selected_playlist_id: Option<i64>,
}

impl NavigationState {
    pub fn navigate(&mut self, page: Page) {
        self.page = page;
        self.selected_playlist_id = None;
    }

    pub fn open_playlist(&mut self, playlist_id: i64) {
        self.page = Page::Library;
        self.selected_playlist_id = Some(playlist_id);
    }
}

impl Default for NavigationState {
    fn default() -> Self {
        Self {
            page: Page::Library,
            selected_playlist_id: None,
        }
    }
}

#[derive(Default)]
pub struct EventState {
    pub search_results: Vec<UiSongData>,
    pub search_loading: bool,
    pub search_error: Option<String>,
    pub queue: Vec<UiQueueData>,
    pub lyrics_mid: Option<String>,
    pub lyrics: Vec<UiLyricData>,
    pub lyrics_loading: bool,
    pub lyrics_error: Option<String>,
    pub login_qr: Option<Vec<u8>>,
    pub login_status: String,
    pub user_name: Option<String>,
    pub auth: UiAuthData,
    pub login_modal_open: bool,
    pub playlists: Vec<UiPlaylistData>,
    pub selected_playlist: Option<i64>,
    pub playlist_tracks: Vec<UiPlaylistTrackData>,
    pub playlist_error: Option<String>,
}

impl EventState {
    pub fn begin_search(&mut self) {
        self.search_loading = true;
        self.search_error = None;
    }

    pub fn open_login_modal(&mut self) {
        self.login_qr = None;
        self.login_status.clear();
        self.login_modal_open = true;
    }

    pub fn close_login_modal(&mut self) {
        self.login_modal_open = false;
        if self.auth.phase != UiLoginPhase::LoggedIn {
            self.login_qr = None;
            self.login_status.clear();
        }
    }

    pub fn apply(&mut self, event: AppEvent) {
        match event {
            AppEvent::SearchDone(results) => {
                self.search_results = results;
                self.search_loading = false;
                self.search_error = None;
            }
            AppEvent::SearchFailed(message) => {
                self.search_loading = false;
                self.search_error = Some(message);
            }
            AppEvent::QueueUpdated(queue) => self.queue = queue,
            AppEvent::LyricsLoading(mid) => {
                if mid.trim().is_empty() {
                    return;
                }
                self.lyrics_mid = Some(mid);
                self.lyrics.clear();
                self.lyrics_loading = true;
                self.lyrics_error = None;
            }
            AppEvent::LyricsLoaded { mid, lines } => {
                if self.lyrics_mid.as_deref() != Some(mid.as_str()) {
                    return;
                }
                self.lyrics = lines;
                self.lyrics_loading = false;
                self.lyrics_error = None;
            }
            AppEvent::LyricsFailed { mid, message } => {
                if self.lyrics_mid.as_deref() != Some(mid.as_str()) {
                    return;
                }
                self.lyrics_loading = false;
                self.lyrics_error = Some(message);
            }
            AppEvent::LoginQr(png) => self.login_qr = Some(png),
            AppEvent::AuthChanged(auth) => {
                self.login_status = auth.message.clone();
                self.user_name =
                    (auth.phase == UiLoginPhase::LoggedIn).then(|| auth.display_name.clone());
                if auth.phase == UiLoginPhase::LoggedIn && auth.message.is_empty() {
                    self.login_qr = None;
                    self.login_modal_open = false;
                } else if matches!(
                    auth.phase,
                    UiLoginPhase::CreatingQr
                        | UiLoginPhase::WaitingScan
                        | UiLoginPhase::WaitingConfirm
                        | UiLoginPhase::Expired
                        | UiLoginPhase::Error
                ) {
                    self.login_modal_open = true;
                }
                self.auth = auth;
            }
            AppEvent::PlaylistsUpdated(playlists) => {
                if self
                    .selected_playlist
                    .is_some_and(|selected| !playlists.iter().any(|row| row.id == selected))
                {
                    self.selected_playlist = None;
                    self.playlist_tracks.clear();
                }
                self.playlists = playlists;
                self.playlist_error = None;
            }
            AppEvent::PlaylistOpened {
                playlist_id,
                tracks,
            } => {
                self.selected_playlist = Some(playlist_id);
                self.playlist_tracks = tracks;
                self.playlist_error = None;
            }
            AppEvent::PlaylistsFailed(message) => {
                self.playlist_error = Some(message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_starts_in_library_and_can_switch_pages() {
        let mut navigation = NavigationState::default();
        assert_eq!(navigation.page, Page::Library);

        navigation.navigate(Page::Lyrics);
        assert_eq!(navigation.page, Page::Lyrics);
    }

    #[test]
    fn every_sidebar_page_has_stable_copy() {
        let expected = [
            (Page::Search, "搜索"),
            (Page::Recommend, "推荐"),
            (Page::Library, "媒体库"),
            (Page::Queue, "播放队列"),
            (Page::Lyrics, "歌词"),
            (Page::Settings, "设置"),
        ];

        for (page, label) in expected {
            assert_eq!(page.label(), label);
        }
    }

    #[test]
    fn content_playlist_selection_is_stable_by_database_id() {
        let mut navigation = NavigationState::default();
        navigation.open_playlist(42);
        assert_eq!(navigation.selected_playlist_id, Some(42));
        assert_eq!(navigation.page, Page::Library);
    }

    #[test]
    fn app_events_replace_view_collections_without_touching_playback_state() {
        let mut state = EventState::default();

        state.apply(AppEvent::SearchDone(vec![UiSongData {
            title: "星茶会".into(),
            artist: "灰澈".into(),
            duration: "04:11".into(),
        }]));
        assert_eq!(state.search_results.len(), 1);
        assert_eq!(state.search_results[0].title, "星茶会");
        assert!(state.search_error.is_none());

        state.apply(AppEvent::QueueUpdated(vec![UiQueueData {
            track_id: "qq:1".into(),
            title: "星茶会".into(),
            artist: "灰澈".into(),
            duration: "04:11".into(),
            is_current: true,
            is_playing: true,
        }]));
        assert!(state.queue[0].is_current);

        state.apply(AppEvent::LyricsLoading("1".into()));
        state.apply(AppEvent::LyricsLoaded {
            mid: "1".into(),
            lines: vec![UiLyricData {
                timestamp_ms: 1_000,
                time: "00:01".into(),
                text: "测试歌词".into(),
                translation: "Test lyric".into(),
            }],
        });
        assert_eq!(state.lyrics_mid.as_deref(), Some("1"));
        assert_eq!(state.lyrics[0].translation, "Test lyric");
        assert!(!state.lyrics_loading);
    }

    #[test]
    fn failures_replace_loading_state_with_a_visible_error() {
        let mut state = EventState::default();
        state.begin_search();
        assert!(state.search_loading);
        assert!(state.search_error.is_none());

        state.apply(AppEvent::SearchFailed("network".into()));
        assert_eq!(state.search_error.as_deref(), Some("network"));

        state.apply(AppEvent::LyricsLoading("mid".into()));
        assert!(state.lyrics_loading);
        state.apply(AppEvent::LyricsFailed {
            mid: "mid".into(),
            message: "missing".into(),
        });
        assert!(!state.lyrics_loading);
        assert_eq!(state.lyrics_error.as_deref(), Some("missing"));
    }

    #[test]
    fn auth_events_drive_modal_and_account_state() {
        let mut state = EventState {
            login_modal_open: true,
            login_qr: Some(vec![1, 2, 3]),
            ..EventState::default()
        };

        state.apply(AppEvent::AuthChanged(UiAuthData::logged_in("10001")));

        assert_eq!(state.auth.phase, UiLoginPhase::LoggedIn);
        assert_eq!(state.user_name.as_deref(), Some("10001"));
        assert!(!state.login_modal_open);
        assert!(state.login_qr.is_none());
    }

    #[test]
    fn playlist_events_replace_only_playlist_state() {
        let mut state = EventState::default();
        state.apply(AppEvent::PlaylistsUpdated(vec![
            hmp_desktop_common::UiPlaylistData {
                id: 1,
                name: "歌单".into(),
                track_count: 2,
                provider: "local".into(),
                relation: "local".into(),
                sync_state: "synced".into(),
            },
        ]));

        assert_eq!(state.playlists.len(), 1);
        assert_eq!(state.playlists[0].name, "歌单");
        assert!(state.search_results.is_empty());
    }

    #[test]
    fn playlist_open_and_failure_update_detail_state() {
        let mut state = EventState::default();
        state.apply(AppEvent::PlaylistOpened {
            playlist_id: 4,
            tracks: vec![hmp_desktop_common::UiPlaylistTrackData {
                source_key: "mid-1".into(),
                title: "一".into(),
                artist: "歌手".into(),
                album: "专辑".into(),
                duration: "03:20".into(),
            }],
        });
        assert_eq!(state.selected_playlist, Some(4));
        assert_eq!(state.playlist_tracks.len(), 1);

        state.apply(AppEvent::PlaylistsFailed("stale".into()));
        assert_eq!(state.playlist_error.as_deref(), Some("stale"));
    }
}
