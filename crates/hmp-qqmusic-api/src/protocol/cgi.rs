//! CGI 请求描述符与批量信封处理（对应上游 `core/request.py` 的
//! `CgiRequest._parse_response` 与 `core/client.py` 的 `_unwrap_cgi_batch`）。

use serde_json::{Value, json};

use crate::error::QqMusicError;

/// CGI 请求描述符。
#[derive(Clone, Debug)]
pub struct CgiRequest {
    /// 模块名（如 `music.search.SearchCgiService`）。
    pub module: String,
    /// 方法名（如 `DoSearchForQQMusicDesktop`）。
    pub method: String,
    /// 业务参数。
    pub param: serde_json::Value,
    /// 自定义公共参数（上游 `comm`），覆盖同名默认 comm 字段。
    pub comm: Option<serde_json::Value>,
    /// 完全用 `comm` 替代默认公共参数，不做合并（上游 `override_comm`）。
    pub override_comm: bool,
    /// 允许的错误码集合；命中时不抛业务错误。
    pub allow_error_codes: Option<Vec<i64>>,
    /// 请求是否需要登录。
    pub require_login: bool,
}

impl CgiRequest {
    /// 构造请求。
    pub fn new(
        module: impl Into<String>,
        method: impl Into<String>,
        param: serde_json::Value,
    ) -> Self {
        Self {
            module: module.into(),
            method: method.into(),
            param,
            comm: None,
            override_comm: false,
            allow_error_codes: None,
            require_login: false,
        }
    }

    /// 标记请求需要登录态（`musicu_request` 校验凭证）。
    pub fn with_require_login(mut self, require: bool) -> Self {
        self.require_login = require;
        self
    }

    /// 设置自定义公共参数（合并进默认 comm；`override` 时完全替换）。
    pub fn with_comm(mut self, comm: Value) -> Self {
        self.comm = Some(comm);
        self
    }

    /// 序列化为 `req_N` 子项。
    pub fn to_req_value(&self) -> Value {
        json!({
            "module": self.module,
            "method": self.method,
            "param": self.param,
        })
    }
}

/// 业务响应错误码映射（上游 `CgiRequest._parse_response`）。
///
/// - `2000` → `SignatureRequired`
/// - `2001` → `Ratelimited`
/// - `1000` / `104401` / `104400` → `CredentialExpired`
/// - 其他非 0 → `QqApi { code, message }`
/// - `0` → `None`（成功）
///
/// 若 `allow_error_codes` 命中，返回 `None`（视为允许）。
pub fn map_business_code(code: i64, allow_error_codes: Option<&[i64]>) -> Option<QqMusicError> {
    if code == 0 {
        return None;
    }
    if let Some(allowed) = allow_error_codes {
        if allowed.contains(&code) {
            return None;
        }
    }
    match code {
        2000 => Some(QqMusicError::SignatureRequired),
        2001 => Some(QqMusicError::Ratelimited),
        1000 | 104401 | 104400 => Some(QqMusicError::CredentialExpired),
        other => Some(QqMusicError::QqApi {
            code: other,
            message: format!("business error code {other}"),
        }),
    }
}

/// 解包 `musicu.fcg` 批量响应外层信封（上游 `_unwrap_cgi_batch`）。
///
/// 校验：
/// - 顶层 `code != 0` → 全局错误（`QqApi`）；
/// - 缺少 `req_0..req_N` 子响应 → `InvalidResponse`。
pub fn unwrap_cgi_batch(body: &Value, expected_count: usize) -> Result<Vec<Value>, QqMusicError> {
    let obj = body
        .as_object()
        .ok_or_else(|| QqMusicError::InvalidResponse("response is not a JSON object".into()))?;

    if let Some(code) = obj.get("code").and_then(|v| v.as_i64()) {
        if code != 0 {
            return Err(QqMusicError::QqApi {
                code,
                message: "global error".into(),
            });
        }
    }

    let mut out = Vec::with_capacity(expected_count);
    for i in 0..expected_count {
        let key = format!("req_{i}");
        let item = obj.get(&key).cloned().ok_or_else(|| {
            QqMusicError::InvalidResponse(format!("missing {key} in CGI response"))
        })?;
        out.push(item);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_value_shape() {
        let req = CgiRequest::new(
            "music.search.SearchCgiService",
            "DoSearchForQQMusicDesktop",
            json!({"query": "周杰伦", "num_per_page": 2, "page_num": 1}),
        );
        assert_eq!(
            serde_json::to_string(&req.to_req_value()).unwrap(),
            r#"{"module":"music.search.SearchCgiService","method":"DoSearchForQQMusicDesktop","param":{"query":"周杰伦","num_per_page":2,"page_num":1}}"#
        );
    }

    #[test]
    fn business_code_zero_is_ok() {
        assert!(map_business_code(0, None).is_none());
    }

    #[test]
    fn business_code_2000_is_signature_required() {
        assert!(matches!(
            map_business_code(2000, None),
            Some(QqMusicError::SignatureRequired)
        ));
    }

    #[test]
    fn business_code_2001_is_rate_limited() {
        assert!(matches!(
            map_business_code(2001, None),
            Some(QqMusicError::Ratelimited)
        ));
    }

    #[test]
    fn business_codes_expire_credential() {
        for code in [1000, 104401, 104400] {
            assert!(
                matches!(
                    map_business_code(code, None),
                    Some(QqMusicError::CredentialExpired)
                ),
                "code {code} should map to CredentialExpired"
            );
        }
    }

    #[test]
    fn unknown_business_code_carries_code() {
        match map_business_code(666, None) {
            Some(QqMusicError::QqApi { code, .. }) => assert_eq!(code, 666),
            other => panic!("expected QqApi, got {other:?}"),
        }
    }

    #[test]
    fn allow_error_codes_suppresses_error() {
        assert!(map_business_code(1000, Some(&[1000])).is_none());
        assert!(map_business_code(1000, Some(&[2001])).is_some());
        assert!(map_business_code(2001, Some(&[1000, 2001])).is_none());
    }

    #[test]
    fn unwrap_extracts_expected_count() {
        let body = json!({
            "code": 0,
            "req_0": {"code": 0, "data": {"song": {"list": []}}},
            "req_1": {"code": 0, "data": {"song": {"list": []}}},
        });
        let out = unwrap_cgi_batch(&body, 2).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["data"]["song"]["list"], json!([]));
    }

    #[test]
    fn unwrap_rejects_global_error() {
        let body = json!({"code": 404, "message": "not found"});
        match unwrap_cgi_batch(&body, 1) {
            Err(QqMusicError::QqApi { code, .. }) => assert_eq!(code, 404),
            other => panic!("expected global QqApi error, got {other:?}"),
        }
    }

    #[test]
    fn unwrap_rejects_missing_subresponse() {
        let body = json!({"code": 0, "req_0": {"code": 0}});
        assert!(matches!(
            unwrap_cgi_batch(&body, 2),
            Err(QqMusicError::InvalidResponse(_))
        ));
    }

    #[test]
    fn unwrap_rejects_non_object() {
        let body = json!([1, 2, 3]);
        assert!(matches!(
            unwrap_cgi_batch(&body, 1),
            Err(QqMusicError::InvalidResponse(_))
        ));
    }
}
