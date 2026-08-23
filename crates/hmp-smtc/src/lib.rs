//! Windows System Media Transport Controls adapter.

pub mod model;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{SmtcError, SmtcService};

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hmp_core::{
        AlbumId, AlbumRef, ArtistId, ArtistRef, CoverRef, LoopMode, PlaybackCapabilities,
        PlaybackState, PlaybackStatus, PlayerCommand, Track, TrackId,
    };

    use crate::model::{
        ProjectedButton, ProjectedStatus, Projection, map_button, map_repeat_request,
    };

    fn sample_playing_state() -> PlaybackState {
        PlaybackState {
            status: PlaybackStatus::Playing,
            current: Some(Track {
                id: TrackId::new("track-1"),
                title: "Song".into(),
                artists: vec![ArtistRef {
                    id: ArtistId::new("artist-1"),
                    name: "Artist".into(),
                }],
                album: Some(AlbumRef {
                    id: AlbumId::new("album-1"),
                    name: "Album".into(),
                }),
                duration: Some(Duration::from_secs(180)),
                cover: Some(CoverRef {
                    url: "https://example.invalid/cover.jpg".into(),
                }),
                url: None,
                available_qualities: vec![],
            }),
            position: Duration::from_secs(45),
            duration: Some(Duration::from_secs(180)),
            can_seek: true,
            loop_mode: LoopMode::List,
            shuffle: true,
            ..PlaybackState::default()
        }
    }

    #[test]
    fn playing_state_projects_metadata_timeline_and_capabilities() {
        let projection = Projection::from_state(
            &sample_playing_state(),
            PlaybackCapabilities {
                can_go_next: true,
                can_go_previous: false,
            },
        );
        assert_eq!(projection.status, ProjectedStatus::Playing);
        assert_eq!(projection.title.as_deref(), Some("Song"));
        assert_eq!(projection.artist.as_deref(), Some("Artist"));
        assert_eq!(projection.album.as_deref(), Some("Album"));
        assert_eq!(projection.position, Duration::from_secs(45));
        assert_eq!(projection.duration, Some(Duration::from_secs(180)));
        assert!(projection.can_seek);
        assert!(projection.can_next);
        assert!(!projection.can_previous);
        assert!(projection.shuffle);
    }

    #[test]
    fn smtc_requests_map_to_domain_commands() {
        assert_eq!(map_button(ProjectedButton::Play), Some(PlayerCommand::Play));
        assert_eq!(map_button(ProjectedButton::Next), Some(PlayerCommand::Next));
        assert_eq!(
            map_repeat_request(0),
            Some(PlayerCommand::SetLoopMode(LoopMode::None))
        );
        assert_eq!(
            map_repeat_request(1),
            Some(PlayerCommand::SetLoopMode(LoopMode::Track))
        );
        assert_eq!(
            map_repeat_request(2),
            Some(PlayerCommand::SetLoopMode(LoopMode::List))
        );
        assert_eq!(map_repeat_request(99), None);
    }
}
