//! Deterministic local content used only by the recommendation UI.

use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::UiFeatureData;

const COVER_SIZE: u32 = 320;
const PALETTES: [[Rgba8Pixel; 4]; 6] = [
    [
        Rgba8Pixel::new(36, 43, 58, 255),
        Rgba8Pixel::new(230, 68, 92, 255),
        Rgba8Pixel::new(247, 190, 73, 255),
        Rgba8Pixel::new(245, 238, 222, 255),
    ],
    [
        Rgba8Pixel::new(15, 76, 92, 255),
        Rgba8Pixel::new(53, 143, 128, 255),
        Rgba8Pixel::new(238, 174, 92, 255),
        Rgba8Pixel::new(250, 239, 218, 255),
    ],
    [
        Rgba8Pixel::new(72, 42, 74, 255),
        Rgba8Pixel::new(180, 70, 99, 255),
        Rgba8Pixel::new(230, 141, 92, 255),
        Rgba8Pixel::new(244, 218, 181, 255),
    ],
    [
        Rgba8Pixel::new(24, 52, 88, 255),
        Rgba8Pixel::new(43, 112, 154, 255),
        Rgba8Pixel::new(104, 179, 176, 255),
        Rgba8Pixel::new(232, 232, 208, 255),
    ],
    [
        Rgba8Pixel::new(52, 56, 48, 255),
        Rgba8Pixel::new(105, 127, 77, 255),
        Rgba8Pixel::new(211, 158, 72, 255),
        Rgba8Pixel::new(237, 225, 190, 255),
    ],
    [
        Rgba8Pixel::new(41, 37, 63, 255),
        Rgba8Pixel::new(105, 79, 141, 255),
        Rgba8Pixel::new(194, 102, 130, 255),
        Rgba8Pixel::new(242, 188, 156, 255),
    ],
];

/// A browse-only recommendation item. It is never treated as account library data.
pub struct UiLibraryData {
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub status: String,
    pub cover: slint::Image,
}

/// Stable local recommendations with generated bitmap covers.
pub fn demo_recommendations() -> Vec<UiLibraryData> {
    let copy = [
        ("精选歌单", "城市夜行", "华语流行与轻电子"),
        ("主题电台", "清晨海岸", "舒缓节奏与原声旋律"),
        ("精选专辑", "旧日来信", "温暖女声与经典旋律"),
        ("主题歌单", "雨后蓝调", "爵士、灵魂乐与蓝调"),
        ("场景推荐", "林间午后", "民谣与自然系器乐"),
        ("编辑精选", "霓虹心事", "独立流行与合成器浪潮"),
    ];

    copy.into_iter()
        .enumerate()
        .map(|(index, (kind, title, subtitle))| UiLibraryData {
            kind: kind.into(),
            title: title.into(),
            subtitle: subtitle.into(),
            status: "demo".into(),
            cover: generated_cover(index),
        })
        .collect()
}

/// User-visible capability record shared by content and settings pages.
pub fn feature_matrix() -> Vec<UiFeatureData> {
    [
        ("登录", "已接入", "QQ 音乐扫码登录与凭据状态"),
        ("搜索", "已接入", "使用 QQ Music Rust API"),
        (
            "播放控制",
            "已接入",
            "播放、暂停、上一首、下一首、Seek、音量",
        ),
        ("队列展示", "已接入", "展示 AppCore 当前真实队列"),
        ("歌词展示", "部分接入", "已接入接口与空状态，按真实返回展示"),
        ("推荐内容", "开发中 / 演示数据", "当前使用本地演示数据"),
        ("收藏与资料库同步", "开发中", "尚未接入账号云端同步"),
    ]
    .into_iter()
    .map(|(name, status, detail)| UiFeatureData {
        name: name.into(),
        status: status.into(),
        detail: detail.into(),
    })
    .collect()
}

fn generated_cover(index: usize) -> slint::Image {
    let palette = PALETTES[index % PALETTES.len()];
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(COVER_SIZE, COVER_SIZE);

    for (offset, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        let x = offset as u32 % COVER_SIZE;
        let y = offset as u32 / COVER_SIZE;
        let diagonal = (x + y + index as u32 * 29) % COVER_SIZE;
        let block = ((x / 80) + (y / 80) + index as u32) % 4;
        let band = diagonal / 80;
        *pixel = palette[((block + band) % 4) as usize];
    }

    slint::Image::from_rgba8(buffer)
}
