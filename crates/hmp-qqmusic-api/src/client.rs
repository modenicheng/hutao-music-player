//! 客户端（对应上游 `qqmusic_api/core/client.py` 的 CGI 执行路径）。
//!
//! # 凭证模型（设计决策）
//!
//! 本 crate **不维护任何全局凭证状态，也不负责凭证轮换**：
//!
//! - 所有需要登录态的请求以 `credential: Option<&Credential>` 参数形式
//!   由调用方显式传入；
//! - 凭证刷新仅通过 `LoginApi::refresh_credential` 显式完成，调用方传入
//!   需要刷新的凭证并自行管理返回的新凭证（支持多凭证场景）；
//! - 调用方负责凭证的存储（keyring 等）与生命周期。

use serde_json::Value;

use crate::config::ClientConfig;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::protocol::cgi::{CgiRequest, map_business_code, unwrap_cgi_batch};
use crate::protocol::comm::{build_web_comm, credential_cookies};
use crate::protocol::search::QuickSearch;

/// QQ 音乐 API 客户端。
///
/// 无内部凭证状态：每次请求的凭证由调用方传入（见[模块文档](self)）。
pub struct QqMusicClient {
    http: reqwest::Client,
    config: ClientConfig,
}

impl QqMusicClient {
    /// 使用默认配置创建客户端。
    pub fn new() -> Self {
        Self::with_config(ClientConfig::default())
    }

    /// 使用自定义配置创建客户端。
    ///
    /// # Panics
    ///
    /// 当配置无法构造 reqwest 客户端时 panic（仅发生在非法配置下，
    /// 例如无法解析的超时值）。
    pub fn with_config(config: ClientConfig) -> Self {
        // 配置非法（如 timeout 无法解析）时 panic，已文档化
        #[allow(clippy::expect_used)]
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .user_agent(config.user_agent.clone())
            .cookie_store(true)
            .build()
            .expect("valid reqwest client config");
        Self { http, config }
    }

    /// 统一的 CGI 请求入口（docs/PROJECT.md §6.3 `musicu_request`）。
    ///
    /// 负责：构造 `comm`、注入 Cookie 与 User-Agent、HTTP 状态检查、
    /// QQ 业务错误码映射、批量响应解包。返回 `req_0` 子响应 JSON。
    ///
    /// `credential` 为请求级凭证：`require_login` 请求必须传入有效凭证，
    /// 否则返回 [`QqMusicError::AuthenticationRequired`]；免登录请求可传 `None`。
    pub async fn musicu_request(
        &self,
        request: &CgiRequest,
        credential: Option<&Credential>,
    ) -> Result<Value, QqMusicError> {
        if request.require_login && !credential.is_some_and(Credential::is_logged_in) {
            return Err(QqMusicError::AuthenticationRequired);
        }

        let comm = build_web_comm(credential.unwrap_or(&Credential::default()));
        let payload = serde_json::json!({
            "comm": comm,
            "req_0": request.to_req_value(),
        });

        let mut req = self
            .http
            .post(format!("{}/cgi-bin/musicu.fcg", self.config.base_url))
            .json(&payload)
            .header("Referer", "https://y.qq.com/");

        // Cookie 注入（上游 prepare_http_kwargs）
        if let Some(cred) = credential {
            let cookies = credential_cookies(cred);
            if !cookies.is_empty() {
                let joined = cookies
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                req = req.header("Cookie", joined);
            }
        }

        tracing::debug!(module = %request.module, method = %request.method, "musicu request");

        let resp = req
            .send()
            .await
            .map_err(|e| QqMusicError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(QqMusicError::Http {
                status: status.as_u16(),
                message: status.to_string(),
            });
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| QqMusicError::InvalidResponse(e.to_string()))?;

        let mut sub_responses = unwrap_cgi_batch(&body, 1)?;
        let sub = sub_responses.remove(0);

        // 业务错误码映射（上游 CgiRequest._parse_response）
        let code = sub.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
        if let Some(err) = map_business_code(code, request.allow_error_codes.as_deref()) {
            return Err(err);
        }

        Ok(sub)
    }

    /// 快速搜索（上游 `SearchApi.quick_search`）。
    ///
    /// 免登录 GET `smartbox_new.fcg`，返回歌曲/专辑/歌手快速匹配。
    pub async fn quick_search(&self, keyword: &str) -> Result<QuickSearch, QqMusicError> {
        let url = format!(
            "{}/splcloud/fcgi-bin/smartbox_new.fcg",
            self.config.content_base_url
        );

        let resp = self
            .http
            .get(url)
            .query(&[("key", keyword), ("format", "json")])
            .header("Referer", "https://y.qq.com/")
            .send()
            .await
            .map_err(|e| QqMusicError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(QqMusicError::Http {
                status: status.as_u16(),
                message: status.to_string(),
            });
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| QqMusicError::InvalidResponse(e.to_string()))?;

        QuickSearch::from_value(&body)
    }
}

impl Default for QqMusicClient {
    fn default() -> Self {
        Self::new()
    }
}
