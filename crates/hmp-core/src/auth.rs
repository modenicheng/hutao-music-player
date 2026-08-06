//! 登录态摘要（docs/PROJECT.md §5.2 `CredentialSummary`）。
//!
//! 供 UI / MPRIS / 状态栏展示，不携带任何敏感字段
//! （凭据本体由 `hmp-storage` 的 keyring 管理）。

use serde::{Deserialize, Serialize};

/// 登录态摘要。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CredentialSummary {
    /// 是否已登录。
    pub logged_in: bool,
    /// 登录用户昵称（未登录为空）。
    pub nickname: String,
    /// 登录用户 UID（未登录为空）。
    pub uid: String,
    /// 登录方式（如 `qq` / `wx` / 空）。
    pub login_type: String,
    /// 是否为绿钻/VIP 会员（影响可用音质）。
    pub is_vip: bool,
    /// 会员到期时间（unix 秒，非会员为 0）。
    pub vip_expire: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_summary_is_logged_out() {
        let s = CredentialSummary::default();
        assert!(!s.logged_in);
        assert!(s.nickname.is_empty());
        assert!(!s.is_vip);
    }

    #[test]
    fn vip_summary_roundtrips() {
        let s = CredentialSummary {
            logged_in: true,
            nickname: "胡桃".into(),
            uid: "123456".into(),
            login_type: "qq".into(),
            is_vip: true,
            vip_expire: 1_800_000_000,
        };
        let v = serde_json::to_value(&s).unwrap();
        let back: CredentialSummary = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }
}
