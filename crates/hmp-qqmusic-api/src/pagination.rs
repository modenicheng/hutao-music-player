//! 统一分页原语（`Page` / `PagedView` / `Paged`）。
//!
//! QQ 音乐各 CGI 的分页参数形态不一（`page`/`num`、`sin`/`cur_page`、
//! `song_begin`/`song_num`、`From`/`Size`、`offset`/`num`、`start`/`count`、
//! `PageNum`/`PageSize`），`has_more` 判定字段也不统一（显式 `hasmore`/
//! `HasMore`、`total` 推算、全然缺失）。本模块提供全 crate 统一的窗口
//! 描述（[`Page`，1 基页号 + 页大小）与响应侧统一视图
//! （[`PagedView`]，经 [`Paged::paged`] 访问），供 CLI 分页浏览与 JSON
//! 输出消费。
//!
//! has_more 归一规则（[`Paged`] 实现须 docstring 注明所用规则）：
//! 1. 服务端显式给 `hasmore`/`HasMore` 字段的（收藏歌单/收藏专辑/歌单
//!    详情/评论/推荐歌单），以服务端为准；
//! 2. 没有显式字段但有总数的（歌手列表/歌手歌曲/专辑/新碟/榜单详情），
//!    按 `total > page.offset() + items.len()` 推算；
//! 3. 两者都缺的，按 `items.len() == page.num` 保守推算
//!    （[`PagedView::conservative`]；当前 crate 内暂无此形态的响应类型，
//!    保留为公开 helper 并以单元测试锚定）。
//!
//! 上游参考：Python 侧的 `core/pagination.py`（PageStrategy/OffsetStrategy
//! 的 has_next 提取器）在 Rust 侧折叠为本模块的三条规则（设计差异已在
//! docs/QQMUSIC_PORTING.md「有意不移植」登记）。

use std::fmt;

/// 默认页大小（QQ 服务端通用单页上限内的保守取值）。
pub const DEFAULT_NUM: u32 = 30;

/// QQ 服务端通用单页条数上限（超出部分会被服务端钳制或忽略；
/// 2026-09-29 逐域实测各分页接口在 ≤100 时行为正常）。
pub const MAX_NUM: u32 = 100;

/// 服务端未提供总数时 [`PagedView::total`] 的占位值。
pub const UNKNOWN_TOTAL: i64 = -1;

/// 分页窗口：1 基页号 + 页大小。
///
/// 由调用方构造并随请求传入 API（各 API 将其派生为 wire 参数，
/// 如 `song_begin = offset()`、`song_num = num`）；响应侧的
/// [`Paged::paged`] 亦以同一窗口回传用于 has_more 推算。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    /// 页号（1 基；构造时钳制 ≥1）。
    pub page: u32,
    /// 每页条数（构造时钳制 1..=[`MAX_NUM`]）。
    pub num: u32,
}

impl Page {
    /// 构造分页窗口：`page` 钳制 ≥1，`num` 钳制 1..=[`MAX_NUM`]。
    #[must_use]
    pub fn new(page: u32, num: u32) -> Self {
        Self {
            page: page.max(1),
            num: num.clamp(1, MAX_NUM),
        }
    }

    /// 首页窗口（`page=1`、`num=[`DEFAULT_NUM`]）。
    #[must_use]
    pub fn first() -> Self {
        Self {
            page: 1,
            num: DEFAULT_NUM,
        }
    }

    /// 下一页窗口（页号 +1，页大小不变；u32 溢出时饱和）。
    #[must_use]
    pub fn next(&self) -> Self {
        Self {
            page: self.page.saturating_add(1),
            num: self.num,
        }
    }

    /// 起始偏移量（0 基）=`(page - 1) * num`，即多数 CGI 的
    /// `begin`/`sin`/`offset`/`start`/`song_begin`/`From` 参数值。
    #[must_use]
    pub fn offset(&self) -> u64 {
        u64::from(self.page.saturating_sub(1)) * u64::from(self.num)
    }
}

impl Default for Page {
    fn default() -> Self {
        Self::first()
    }
}

impl From<(u32, u32)> for Page {
    fn from((page, num): (u32, u32)) -> Self {
        Self::new(page, num)
    }
}

impl fmt::Display for Page {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "page {} ({} /page)", self.page, self.num)
    }
}

/// 统一分页视图：某响应在 `page` 窗口下的条目切片与续页判定。
///
/// 条目为借用切片（不克隆）；`total` 为服务端总数（未提供时为
/// [`UNKNOWN_TOTAL`]）；`has_more` 按模块级文档的三条归一规则之一得出。
pub struct PagedView<'a, T> {
    /// 当前页条目。
    pub items: &'a [T],
    /// 服务端总数；未提供时为 [`UNKNOWN_TOTAL`]。
    pub total: i64,
    /// 是否还有下一页。
    pub has_more: bool,
    /// 构造本视图时使用的窗口（即发起请求时传入的 [`Page`]）。
    pub page: Page,
}

impl<'a, T> PagedView<'a, T> {
    /// 下一页窗口；`has_more == false` 时返回 `None`。
    #[must_use]
    pub fn next_page(&self) -> Option<Page> {
        if self.has_more {
            Some(self.page.next())
        } else {
            None
        }
    }

    /// 保守归一构造（规则 3）：服务端既无显式 hasmore 字段也无总数时，
    /// 按 `items.len() == page.num` 推算——满页则假设还有下一页。
    ///
    /// 注意这是保守推算：服务端恰好返回整页但已是末页时会多取一次空页
    /// （空页 `items.len() == 0 != page.num` 即终止）。
    #[must_use]
    pub fn conservative(items: &'a [T], page: Page) -> Self {
        Self {
            has_more: items.len() as u64 == u64::from(page.num),
            items,
            total: UNKNOWN_TOTAL,
            page,
        }
    }
}

/// 分页响应的统一视图访问（全 crate 统一形态：trait + `paged` 方法）。
///
/// 每个分页响应类型实现本 trait；实现处的 docstring 须注明所用
/// has_more 归一规则（模块级文档的三条规则之一）。
pub trait Paged {
    /// 当前页条目类型。
    type Item;

    /// 以发起请求时的窗口 `page` 构造统一分页视图。
    fn paged(&self, page: Page) -> PagedView<'_, Self::Item>;
}
