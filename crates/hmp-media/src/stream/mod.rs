//! 进程内随机访问解密流：CDN 探测 → QMC2 尾部解析 → 流密码 →
//! 按需拉取分块解密，以同步 `Read + Seek` reader 直供播放器。
//!
//! 若 CDN 不支持 `Range`，自动回退 [`crate::decrypt`] 全量下载-解密-缓存
//! （`file://` URI）。取代历史上的 `127.0.0.1` 回环 HTTP 代理：daemon 与
//! 播放器同进程后，TCP/HTTP 回环纯属开销且是系统代理劫持事故面。

mod reader;
pub(crate) mod source;

pub use source::{PreparedMedia, cdn_client, prepare_media};
