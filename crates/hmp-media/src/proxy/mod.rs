//! 极简本地 HTTP 代理：接收播放器 Range 请求，从 CDN 拉取并
//! 解密 QMC2 流，按需返回明文音频数据。
//!
//! Task 1 仅实现 Range 头解析与 HTTP/1.1 骨架；Task 2 补充
//! [`source`] 的真实 CDN + 解密管道。

pub mod http;
pub mod range;
