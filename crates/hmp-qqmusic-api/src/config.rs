//! 客户端配置（docs/PROJECT.md §6.3）。

use std::time::Duration;

/// 客户端配置。
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// HTTP 请求超时。
    pub timeout: Duration,
    /// User-Agent。
    pub user_agent: String,
    /// 可重试错误的最大重试次数。
    pub max_retries: usize,
    /// CGI API 基础地址（默认官方 `https://u.y.qq.com`）。
    pub base_url: String,
    /// 内容接口基础地址（默认官方 `https://c.y.qq.com`，如 smartbox 搜索）。
    pub content_base_url: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            max_retries: 2,
            base_url: DEFAULT_BASE_URL.to_owned(),
            content_base_url: DEFAULT_CONTENT_BASE_URL.to_owned(),
        }
    }
}

/// 默认 CGI API 基础地址。
pub const DEFAULT_BASE_URL: &str = "https://u.y.qq.com";

/// 默认内容接口基础地址。
pub const DEFAULT_CONTENT_BASE_URL: &str = "https://c.y.qq.com";

/// 默认 User-Agent（与上游 WEB 平台一致，docs/QQMUSIC_PORTING.md）。
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
