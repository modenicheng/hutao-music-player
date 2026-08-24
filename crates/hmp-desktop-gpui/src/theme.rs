pub const ACCENT: u32 = 0xfa2d55;
pub const BACKGROUND: u32 = 0x20222e;
pub const TEXT_PRIMARY: u32 = 0xf7f7fa;
pub const FONT_FAMILY: &str = "Noto Sans CJK SC";

pub mod layout {
    #[allow(unused_imports)]
    pub use crate::ui_contract::{
        COMPACT_TOP_BAR_BELOW, FLOATING_PLAYER_BOTTOM, FLOATING_PLAYER_RADIUS,
        FLOATING_PLAYER_WIDTH, FLOATING_PLAYER_WIDTH_COMPACT, HIDE_LYRICS_BELOW, LYRICS_WIDTH,
        RegularLayout, SIDEBAR_GUTTER, SIDEBAR_WIDTH, TOP_BAR_HEIGHT, TOP_BAR_SEARCH_WIDTH,
        TOP_BAR_SEARCH_WIDTH_COMPACT, floating_player_width, now_playing_cover_size,
        regular_layout, top_bar_search_width,
    };
}
