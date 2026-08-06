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
    /// QQ 授权登录域（上游 `ssl.ptlogin2.qq.com`，ptqrshow/ptqrlogin）。
    pub login_ptlogin2_url: String,
    /// QQ 授权签名域（上游 `ssl.ptlogin2.graph.qq.com`，check_sig）。
    pub login_graph_url: String,
    /// QQ 开放平台授权域（上游 `graph.qq.com`，oauth2 authorize）。
    pub login_oauth_url: String,
    /// 个人主页检查域（上游 `c6.y.qq.com`，check_expired）。
    pub login_profile_url: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            max_retries: 2,
            base_url: DEFAULT_BASE_URL.to_owned(),
            content_base_url: DEFAULT_CONTENT_BASE_URL.to_owned(),
            login_ptlogin2_url: DEFAULT_LOGIN_PTLOGIN2_URL.to_owned(),
            login_graph_url: DEFAULT_LOGIN_GRAPH_URL.to_owned(),
            login_oauth_url: DEFAULT_LOGIN_OAUTH_URL.to_owned(),
            login_profile_url: DEFAULT_LOGIN_PROFILE_URL.to_owned(),
        }
    }
}

/// 默认 CGI API 基础地址。
pub const DEFAULT_BASE_URL: &str = "https://u.y.qq.com";

/// 默认内容接口基础地址。
pub const DEFAULT_CONTENT_BASE_URL: &str = "https://c.y.qq.com";

/// 默认 QQ 授权登录域。
pub const DEFAULT_LOGIN_PTLOGIN2_URL: &str = "https://ssl.ptlogin2.qq.com";

/// 默认 QQ 授权签名域。
pub const DEFAULT_LOGIN_GRAPH_URL: &str = "https://ssl.ptlogin2.graph.qq.com";

/// 默认 QQ 开放平台授权域。
pub const DEFAULT_LOGIN_OAUTH_URL: &str = "https://graph.qq.com";

/// 默认个人主页检查域。
pub const DEFAULT_LOGIN_PROFILE_URL: &str = "https://c6.y.qq.com";

/// 默认 User-Agent（与上游 WEB 平台一致，docs/QQMUSIC_PORTING.md）。
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
