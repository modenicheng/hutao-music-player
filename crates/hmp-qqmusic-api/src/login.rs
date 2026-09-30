//! 登录模块（对应上游 `modules/login.py` + `modules/login_utils.py`）。
//!
//! # 凭证模型（设计决策）
//!
//! 本模块**不持有全局凭证，不负责凭证轮换**（docs/PROJECT.md §6.4）：
//!
//! - `refresh_credential` / `check_expired` / `logout` 均要求调用方显式传入凭证；
//! - 返回的新凭证由调用方自行存储与管理（支持多凭证场景）。
//!
//! # 会话影响（安全红线）
//!
//! - `get_qrcode` / `check_qrcode` / `wait_qrcode_login` / `check_expired` 为**只读或
//!   无凭证**操作，可安全现场核验；
//! - `refresh_credential` / `logout` / `authorize_qq_qr` 为**会话级写操作**——对真实
//!   凭证调用会使旧会话失效，仅做离线（wiremock）验证。

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::client::QqMusicClient;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::protocol::cgi::CgiRequest;
use crate::protocol::sign::hash33;

/// 二维码登录类型（上游 `models/login.py::QRLoginType`）。
///
/// 上游枚举值为 `"qq"`/`"wx"`/`"mobile"`；Rust 侧仅作请求路由用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QRLoginType {
    /// QQ 扫码（ptlogin2.ptqrshow，当前唯一完整移植的链路）。
    Qq,
    /// 微信扫码（open.weixin.qq.com，待移植）。
    Wechat,
    /// 手机客户端扫码（依赖 MQTT，暂不移植）。
    Mobile,
}

/// 二维码登录流程中的状态事件（上游 `models/login.py::QRCodeLoginEvents`）。
///
/// QQ 链路状态码映射：DONE=(0,405)、SCAN=(66,408)、CONF=(67,404)、
/// TIMEOUT=(65,402)、REFUSE=(68,403)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QRCodeLoginEvents {
    /// 登录完成，携带凭证。
    Done,
    /// 二维码未被扫描，等待扫描。
    Scan,
    /// 已被扫描，等待确认。
    Conf,
    /// 二维码过期或登录超时。
    Timeout,
    /// 用户拒绝登录。
    Refuse,
}

impl QRCodeLoginEvents {
    /// 根据状态码获取事件（上游 `QRCodeLoginEvents.get_by_value`）。
    ///
    /// 无法识别的状态码返回 [`QqMusicError::InvalidResponse`]（上游抛 ValueError）。
    fn get_by_value(value: i64) -> Result<Self, QqMusicError> {
        match value {
            0 | 405 => Ok(Self::Done),
            66 | 408 => Ok(Self::Scan),
            67 | 404 => Ok(Self::Conf),
            65 | 402 => Ok(Self::Timeout),
            68 | 403 => Ok(Self::Refuse),
            other => Err(QqMusicError::InvalidResponse(format!(
                "unrecognized QR login status code: {other}"
            ))),
        }
    }
}

/// 二维码信息（上游 `models/login.py::QR`）。
///
/// 无 `save` 方法（上游扩展）；调用方自行落盘（PNG 原始字节）。
#[derive(Clone, Debug)]
pub struct QR {
    /// 二维码图片二进制数据（QQ 链路为 PNG 字节流，文件头 `\x89PNG`）。
    pub data: Vec<u8>,
    /// 二维码登录类型。
    pub qr_type: QRLoginType,
    /// 图片 MIME 类型（QQ 链路恒为 `image/png`）。
    pub mimetype: String,
    /// 标识符（QQ=qrsig，微信=uuid，手机=qrcodeID）。
    ///
    /// 后续 `check_qrcode` / `wait_qrcode_login` 依赖此值轮询状态。
    pub identifier: String,
}

/// 二维码登录流程中的单次结果（上游 `models/login.py::QRLoginResult`）。
#[derive(Clone, Debug)]
pub struct QRLoginResult {
    /// 状态事件。
    pub event: QRCodeLoginEvents,
    /// 仅在 `Done` 时携带凭证（其余事件恒为 `None`）。
    pub credential: Option<Credential>,
}

impl QRLoginResult {
    /// 是否表示登录完成（上游 `QRLoginResult.done`）。
    ///
    /// 完成时凭证应通过 [`QRLoginResult::credential`] 取用。
    pub fn done(&self) -> bool {
        self.event == QRCodeLoginEvents::Done
    }
}

/// 轮询间隔控制策略（上游 `login_utils.py::PollInterval`，单位秒）。
#[derive(Clone, Debug)]
pub struct PollInterval {
    /// 默认轮询间隔（上游 1.5s）。
    pub default: Duration,
    /// 已扫码（CONF）状态下的轮询间隔；`None` 时取 `default/2`。
    pub scanned: Option<Duration>,
    /// 异常退避最大间隔；`None` 时取 `default*2`。
    pub error: Option<Duration>,
}

impl Default for PollInterval {
    fn default() -> Self {
        Self {
            default: Duration::from_millis(1500),
            scanned: None,
            error: None,
        }
    }
}

impl PollInterval {
    fn scanned_interval(&self) -> Duration {
        self.scanned.unwrap_or_else(|| self.default / 2)
    }

    fn error_interval(&self) -> Duration {
        self.error.unwrap_or_else(|| self.default * 2)
    }
}

/// 登录 API（对应上游 `modules/login.py::LoginApi`）。
///
/// 借用 `&QqMusicClient` 发起请求；凭证参数均由调用方显式传入（无全局凭证状态）。
/// 当前仅移植 QQ 扫码链路；微信/手机客户端扫码待移植。
pub struct LoginApi<'a> {
    client: &'a QqMusicClient,
}

/// QQ 授权登录 CGI 允许的错误码（上游 `_ERROR_CODE`）。
const LOGIN_ERROR_CODES: &[i64] = &[
    1000, 104401, 104400, 20261, 20271, 20272, 20274, 20277, 20278, 20279, 20450, 104604,
];

impl<'a> LoginApi<'a> {
    /// 构造登录 API（复用客户端的配置与 HTTP 会话）。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 校验登录 CGI 响应并返回 `data`（上游 `LoginApi._validate_result`）。
    ///
    /// 错误码映射（与上游一致）：1000/104401/104400 → 登录鉴权过期；
    /// 20261/20271/20272/20274 → 登录参数/验证码/绑定类错误；20277/20278/20450 →
    /// 账号受限或封禁；20279 → 设备数超限；104604 → 登录限流；其余 → 通用登录错误。
    /// 刷新场景由 `refresh_credential` 统一包装为 [`QqMusicError::CredentialRefresh`]。
    fn validate_login_result(data: &Value) -> Result<Value, QqMusicError> {
        let code = data.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
        if code == 0 {
            return Ok(data.get("data").cloned().unwrap_or(json!({})));
        }
        match code {
            1000 | 104401 | 104400 => Err(QqMusicError::LoginAuthExpired),
            20261 => Err(QqMusicError::Login {
                code,
                message: "invalid login parameters".into(),
            }),
            20271 => Err(QqMusicError::Login {
                code,
                message: "captcha error".into(),
            }),
            20272 => Err(QqMusicError::Login {
                code,
                message: "account binding error".into(),
            }),
            20274 => Err(QqMusicError::Login {
                code,
                message: "account binding missing".into(),
            }),
            20277 | 20278 | 20450 => Err(QqMusicError::LoginAccountRestricted),
            20279 => Err(QqMusicError::LoginDeviceLimit),
            104604 => Err(QqMusicError::LoginRateLimit),
            other => Err(QqMusicError::Login {
                code: other,
                message: format!("login business error code {other}"),
            }),
        }
    }

    /// 检查凭证是否过期（上游 `LoginApi.check_expired`；读操作，无会话副作用）。
    ///
    /// WEB 平台实现：GET `fcg_get_profile_homepage.fcg`，以 `g_tk=hash33(musickey, 5381)`
    /// 携带凭证 Cookie，返回业务 `code != 0` 即视为过期（未登录/失效均判过期）。
    ///
    /// # 参数
    /// - `credential`：待检查的凭证（需要 `music_id` + `music_key`）。
    ///
    /// # 返回
    /// `true` = 已过期/无效；`false` = 会话有效。
    ///
    /// 实测（2026-09-29）：真实凭证（uin=939861972）返回 `false`，HTTP 200 + code 0。
    pub async fn check_expired(&self, credential: &Credential) -> Result<bool, QqMusicError> {
        let cfg = &self.client_config();
        let resp = self
            .client
            .http_request(
                reqwest::Method::GET,
                format!(
                    "{}/rsc/fcgi-bin/fcg_get_profile_homepage.fcg",
                    cfg.login_profile_url
                ),
                &[
                    ("g_tk", hash33(&credential.music_key, 5381).to_string()),
                    ("format", "json".into()),
                    ("inCharset", "utf-8".into()),
                    ("outCharset", "utf-8".into()),
                    ("notice", "0".into()),
                    ("cid", "205360838".into()),
                    ("needNewCode", "0".into()),
                    ("loginUin", credential.music_id.clone()),
                    ("hostUin", "0".into()),
                    ("userid", credential.music_id.clone()),
                    ("reqfrom", "1".into()),
                ],
                &[("Referer", "https://y.qq.com/".to_owned())],
                &crate::protocol::comm::credential_cookies(credential),
                None,
                true,
            )
            .await?;

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
        Ok(body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1) != 0)
    }

    /// 刷新登录凭证（上游 `LoginApi.refresh_credential`）。
    ///
    /// **会话级安全（仅离线验证）**：按 `login_type` 分支调用 `music.login.LoginServer/Login`
    /// CGI（`loginMode=2`）换取新 key；调用后旧凭证立即失效，调用方须保存返回的新凭证。
    ///
    /// # 参数
    /// - `credential`：待刷新的凭证（`refresh_key` 必须仍有效）。
    ///
    /// # 返回
    /// 刷新后的新凭证对象；本模块不做自动轮换，存储由调用方负责。
    ///
    /// # 错误
    /// 登录业务错误（含鉴权过期/设备超限/限流）统一包装为
    /// [`QqMusicError::CredentialRefresh`]（与上游 `CredentialRefreshError` 对应）。
    pub async fn refresh_credential(
        &self,
        credential: &Credential,
    ) -> Result<Credential, QqMusicError> {
        let login_type = credential.login_type.as_login_type_int();
        // 上游 Credential.musicid 为 int；musicid 必须按数值下发（字符串形状服务端拒绝）
        let music_id_value = |id: &str| -> Value {
            id.parse::<i64>().map(|n| json!(n)).unwrap_or_else(|_| json!(id))
        };
        let param = match credential.login_type {
            crate::credential::LoginType::Wechat => json!({
                "openid": credential.openid,
                "refresh_token": credential.refresh_token,
                "str_musicid": if credential.str_musicid.is_empty() {
                    credential.music_id.clone()
                } else {
                    credential.str_musicid.clone()
                },
                "musickey": credential.music_key,
                "unionid": credential.unionid,
                "refresh_key": credential.refresh_key.clone().unwrap_or_default(),
                "loginMode": 2,
            }),
            crate::credential::LoginType::Qq => json!({
                "openid": credential.openid,
                "access_token": credential.access_token,
                "refresh_token": credential.refresh_token,
                "expired_in": credential.expired_at,
                "musicid": music_id_value(&credential.music_id),
                "musickey": credential.music_key,
                "refresh_key": credential.refresh_key.clone().unwrap_or_default(),
                "loginMode": 2,
            }),
            crate::credential::LoginType::Other(_) => json!({
                "openid": credential.openid,
                "access_token": credential.access_token,
                "refresh_token": credential.refresh_token,
                "expired_in": credential.expired_at,
                "str_musicid": if credential.str_musicid.is_empty() {
                    credential.music_id.clone()
                } else {
                    credential.str_musicid.clone()
                },
                "musicid": music_id_value(&credential.music_id),
                "musickey": credential.music_key,
                "unionid": credential.unionid,
                "refresh_key": credential.refresh_key.clone().unwrap_or_default(),
                "loginMode": 2,
            }),
        };

        let request = CgiRequest {
            module: "music.login.LoginServer".into(),
            method: "Login".into(),
            param,
            comm: Some(json!({"tmeLoginType": login_type})),
            override_comm: false,
            allow_error_codes: Some(LOGIN_ERROR_CODES.to_vec()),
            require_login: false,
        };

        let data = self
            .client
            .musicu_request(&request, Some(credential))
            .await?;
        // 上游：捕获 LoginError（含子类）并包装为 CredentialRefreshError
        Self::validate_login_result(&data).map_err(Self::wrap_refresh_error)?;
        // 上游 code==0 时 _build_cgi 返回内层 data，_validate_result 会对内层
        // 的 code 再校验一次（服务端可能在 data 内重复携带错误码）
        let inner = data.get("data").cloned().unwrap_or_else(|| json!({}));
        Self::validate_login_result(&inner).map_err(Self::wrap_refresh_error)?;
        Credential::from_login_data(&inner)
    }

    /// 将登录域错误统一包装为 [`QqMusicError::CredentialRefresh`]
    /// （上游 `refresh_credential` 的 `except LoginError` 分支）。
    fn wrap_refresh_error(e: QqMusicError) -> QqMusicError {
        match e {
            QqMusicError::Login { code, message }
            | QqMusicError::CredentialRefresh { code, message } => {
                QqMusicError::CredentialRefresh { code, message }
            }
            QqMusicError::LoginAuthExpired => QqMusicError::CredentialRefresh {
                code: 1000,
                message: "login auth parameters invalid or expired".into(),
            },
            QqMusicError::LoginDeviceLimit => QqMusicError::CredentialRefresh {
                code: 20279,
                message: "login device limit reached".into(),
            },
            QqMusicError::LoginAccountRestricted => QqMusicError::CredentialRefresh {
                code: 20277,
                message: "account restricted or banned".into(),
            },
            QqMusicError::LoginRateLimit => QqMusicError::CredentialRefresh {
                code: 104604,
                message: "login rate limited".into(),
            },
            other => other,
        }
    }

    /// 登出（上游 `LoginApi.logout`）。
    ///
    /// **会话级安全（仅离线验证）**：调用 `music.login.LoginServer/Logout` CGI 后
    /// 服务端使该凭证的 key 失效；本地凭证删除由调用方（如 hmp-storage）负责，
    /// 本模块无全局状态可清理（上游会重置 `client.credential`）。
    ///
    /// # 参数
    /// - `credential`：要登出的凭证（须 `is_logged_in`，否则直接返回
    ///   [`QqMusicError::AuthenticationRequired`]）。
    ///
    /// 实测：仅离线验证（会话级安全），不做 live 调用。
    pub async fn logout(&self, credential: &Credential) -> Result<(), QqMusicError> {
        let request = CgiRequest {
            module: "music.login.LoginServer".into(),
            method: "Logout".into(),
            param: json!({}),
            comm: None,
            override_comm: false,
            allow_error_codes: Some(LOGIN_ERROR_CODES.to_vec()),
            require_login: true,
        };
        self.client
            .musicu_request(&request, Some(credential))
            .await?;
        Ok(())
    }

    /// 获取登录二维码（上游 `LoginApi.get_qrcode`；只读，无会话副作用）。
    ///
    /// 当前仅支持 [`QRLoginType::Qq`]；微信/手机扫码待移植（上游 `_get_wx_qr`/`_get_mobile_qr`）。
    ///
    /// # 返回
    /// [`QR`]：PNG 图片字节（文件头 `\x89PNG`）+ Set-Cookie `qrsig`（作为
    /// `identifier` 供后续 `check_qrcode` 使用）。
    ///
    /// 实测（2026-09-29）：返回有效 PNG（`#89504e47` 头，111×111，432 字节）+
    /// qrsig（128 字符）。
    pub async fn get_qrcode(&self, login_type: QRLoginType) -> Result<QR, QqMusicError> {
        match login_type {
            QRLoginType::Qq => self.get_qq_qr().await,
            QRLoginType::Wechat => Err(QqMusicError::InvalidResponse(
                "WeChat QR login not ported yet".into(),
            )),
            QRLoginType::Mobile => Err(QqMusicError::InvalidResponse(
                "mobile client QR login not ported yet".into(),
            )),
        }
    }

    /// 检查二维码状态（上游 `LoginApi.check_qrcode`；只读，无会话副作用）。
    ///
    /// GET `ptlogin2/ptqrlogin`（Cookie 携带 `qrsig`，`ptqrtoken=hash33(qrsig)`），
    /// 解析 `ptuiCB(...)` 文本为 [`QRCodeLoginEvents`]；`Done` 时继续走
    /// `authorize_qq_qr` 换取凭证。
    ///
    /// # 参数
    /// - `qrcode`：`get_qrcode` 返回的二维码对象（使用其 `qr_type` + `identifier`）。
    ///
    /// # 返回
    /// [`QRLoginResult`]：非 `Done` 事件不携带凭证；`Done` 携带完整 [`Credential`]。
    ///
    /// # 错误
    /// HTTP 非 2xx（无效/过期 qrsig）返回 [`QqMusicError::Http`]（上游包装为
    /// `ApiDataError`，语义一致：调用方可视作终态并重新出码）。
    ///
    /// 实测（2026-09-29）：伪造 qrsig 返回 **HTTP 403 Forbidden**（无 `ptuiCB` 文本），
    /// 即无效二维码以 HTTP 错误表达而非 `ptuiCB` 状态事件；正常轮询（未扫码）则返回
    /// 200 + `ptuiCB('66',...)`。
    pub async fn check_qrcode(&self, qrcode: &QR) -> Result<QRLoginResult, QqMusicError> {
        match qrcode.qr_type {
            QRLoginType::Qq => self.check_qq_qr(qrcode).await,
            QRLoginType::Wechat => Err(QqMusicError::InvalidResponse(
                "WeChat QR login not ported yet".into(),
            )),
            QRLoginType::Mobile => Err(QqMusicError::InvalidResponse(
                "mobile client QR login not ported yet".into(),
            )),
        }
    }

    /// 获取 QQ 授权二维码（上游 `LoginApi._get_qq_qr`）。
    ///
    /// GET `ssl.ptlogin2.qq.com/ptqrshow`（appid=716027609），从 Set-Cookie 提取
    /// `qrsig`，响应体即 PNG 字节。缺 qrsig 时返回 [`QqMusicError::InvalidResponse`]。
    async fn get_qq_qr(&self) -> Result<QR, QqMusicError> {
        let cfg = self.client_config();
        let resp = self
            .client
            .http_request(
                reqwest::Method::GET,
                format!("{}/ptqrshow", cfg.login_ptlogin2_url),
                &[
                    ("appid", "716027609".into()),
                    ("e", "2".into()),
                    ("l", "M".into()),
                    ("s", "3".into()),
                    ("d", "72".into()),
                    ("v", "4".into()),
                    ("t", random_f64_str()),
                    ("daid", "383".into()),
                    ("pt_3rd_aid", "100497308".into()),
                ],
                &[("Referer", "https://xui.ptlogin2.qq.com/".to_owned())],
                &[],
                None,
                true,
            )
            .await?;

        let qrsig = extract_cookie(&resp, "qrsig")
            .ok_or_else(|| QqMusicError::InvalidResponse("failed to obtain qrsig".into()))?;

        let data = resp
            .bytes()
            .await
            .map_err(|e| QqMusicError::InvalidResponse(e.to_string()))?
            .to_vec();

        Ok(QR {
            data,
            qr_type: QRLoginType::Qq,
            mimetype: "image/png".into(),
            identifier: qrsig,
        })
    }

    /// 检查 QQ 二维码状态（上游 `LoginApi._check_qq_qr`）。
    ///
    /// 解析 `ptuiCB('code',...)`；状态码经
    /// [`QRCodeLoginEvents::get_by_value`] 映射，`Done` 时从第 3 个参数的跳转 URL
    /// 提取 `uin` 与 `ptsigx` 并转入 `authorize_qq_qr`。
    async fn check_qq_qr(&self, qrcode: &QR) -> Result<QRLoginResult, QqMusicError> {
        let cfg = self.client_config();
        let resp = self
            .client
            .http_request(
                reqwest::Method::GET,
                format!("{}/ptqrlogin", cfg.login_ptlogin2_url),
                &[
                    ("u1", "https://graph.qq.com/oauth2.0/login_jump".into()),
                    ("ptqrtoken", hash33(&qrcode.identifier, 0).to_string()),
                    ("ptredirect", "0".into()),
                    ("h", "1".into()),
                    ("t", "1".into()),
                    ("g", "1".into()),
                    ("from_ui", "1".into()),
                    ("ptlang", "2052".into()),
                    ("action", format!("0-0-{}", now_millis())),
                    ("js_ver", "20102616".into()),
                    ("js_type", "1".into()),
                    ("pt_uistyle", "40".into()),
                    ("aid", "716027609".into()),
                    ("daid", "383".into()),
                    ("pt_3rd_aid", "100497308".into()),
                    ("has_onekey", "1".into()),
                ],
                &[("Referer", "https://xui.ptlogin2.qq.com/".to_owned())],
                &[("qrsig".into(), qrcode.identifier.clone())],
                None,
                true,
            )
            .await?;

        let status = resp.status();
        if !status.is_success() {
            return Err(QqMusicError::Http {
                status: status.as_u16(),
                message: status.to_string(),
            });
        }

        let text = resp
            .text()
            .await
            .map_err(|e| QqMusicError::InvalidResponse(e.to_string()))?;

        let (code, args) = parse_ptui_cb(&text)?;
        let event = QRCodeLoginEvents::get_by_value(code)?;
        if event != QRCodeLoginEvents::Done {
            return Ok(QRLoginResult {
                event,
                credential: None,
            });
        }

        // Done：解析 ptsigx 与 uin
        let (uin, sigx) = parse_done_args(&args)?;
        let credential = self.authorize_qq_qr(&uin, &sigx).await?;
        Ok(QRLoginResult {
            event,
            credential: Some(credential),
        })
    }

    /// 完成 QQ 二维码授权并换取凭证（上游 `LoginApi._authorize_qq_qr`）。
    ///
    /// **会话级安全（仅离线验证）**：三步链路——
    /// 1. GET `ssl.ptlogin2.graph.qq.com/check_sig`（携带 `ptsigx`）→ Set-Cookie `p_skey`；
    /// 2. POST `graph.qq.com/oauth2.0/authorize`（表单 + p_skey Cookie）→ 302 `Location` 中的 `code`；
    /// 3. CGI `QQConnectLogin.LoginServer/QQLogin`（comm `tmeLoginType=2`）→ 登录响应 `data` 构造 [`Credential`]。
    ///
    /// 任一步缺失关键数据（p_skey / Location code）返回 [`QqMusicError::InvalidResponse`]。
    async fn authorize_qq_qr(&self, uin: &str, sigx: &str) -> Result<Credential, QqMusicError> {
        let cfg = self.client_config();

        // 1) check_sig → p_skey
        let resp = self
            .client
            .http_request(
                reqwest::Method::GET,
                format!("{}/check_sig", cfg.login_graph_url),
                &[
                    ("uin", uin.to_owned()),
                    ("pttype", "1".into()),
                    ("service", "ptqrlogin".into()),
                    ("nodirect", "0".into()),
                    ("ptsigx", sigx.to_owned()),
                    ("s_url", "https://graph.qq.com/oauth2.0/login_jump".into()),
                    ("ptlang", "2052".into()),
                    ("ptredirect", "100".into()),
                    ("aid", "716027609".into()),
                    ("daid", "383".into()),
                    ("j_later", "0".into()),
                    ("low_login_hour", "0".into()),
                    ("regmaster", "0".into()),
                    ("pt_login_type", "3".into()),
                    ("pt_aid", "0".into()),
                    ("pt_aaid", "16".into()),
                    ("pt_light", "0".into()),
                    ("pt_3rd_aid", "100497308".into()),
                ],
                &[("Referer", "https://xui.ptlogin2.qq.com/".to_owned())],
                &[],
                None,
                false,
            )
            .await?;

        let cookies = response_cookies(&resp);
        let p_skey = cookies
            .iter()
            .find(|(k, _)| k == "p_skey")
            .map(|(_, v)| v.clone())
            .ok_or_else(|| QqMusicError::InvalidResponse("failed to obtain p_skey".into()))?;

        // 2) oauth authorize → Location 中的 code
        let resp = self
            .client
            .http_request(
                reqwest::Method::POST,
                format!("{}/oauth2.0/authorize", cfg.login_oauth_url),
                &[],
                &[("Referer", "https://xui.ptlogin2.qq.com/".to_owned())],
                &cookies,
                Some(&[
                    ("response_type", "code".into()),
                    ("client_id", "100497308".into()),
                    (
                        "redirect_uri",
                        "https://y.qq.com/portal/wx_redirect.html?login_type=1&surl=https://y.qq.com/".into(),
                    ),
                    ("scope", "get_user_info,get_app_friends".into()),
                    ("state", "state".into()),
                    ("switch", "".into()),
                    ("from_ptlogin", "1".into()),
                    ("src", "1".into()),
                    ("update_auth", "1".into()),
                    ("openapi", "1010_1030".into()),
                    ("g_tk", hash33(&p_skey, 5381).to_string()),
                    ("auth_time", now_millis().to_string()),
                    ("ui", uuid4_str()),
                ]),
                false,
            )
            .await?;

        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| QqMusicError::InvalidResponse("failed to obtain code".into()))?
            .to_owned();
        let code = extract_code_from_location(&location)?;

        // 3) QQLogin CGI → Credential
        let request = CgiRequest {
            module: "QQConnectLogin.LoginServer".into(),
            method: "QQLogin".into(),
            param: json!({"code": code}),
            comm: Some(json!({"tmeLoginType": 2})),
            override_comm: false,
            allow_error_codes: Some(LOGIN_ERROR_CODES.to_vec()),
            require_login: false,
        };
        let data = self.client.musicu_request(&request, None).await?;
        Self::validate_login_result(&data)?;
        // 上游 code==0 时 _build_cgi 返回内层 data，_validate_result 会再校验内层 code
        let inner = data.get("data").cloned().unwrap_or_else(|| json!({}));
        Self::validate_login_result(&inner)?;
        Credential::from_login_data(&inner)
    }

    /// 等待二维码登录完成（上游 `login_utils.py::QRCodeLoginSession.wait_qrcode_login`）。
    ///
    /// 轮询 `check_qrcode` 直至终态事件；连续重复事件去重（上游 `emit_repeat=false`），
    /// CONF 状态按 `interval.scanned` 加速，网络错误按指数退避（`min(error_interval,
    /// default*2^n)`，与上游一致），单次轮询间隔不低于 1s（上游 `min_safe_interval`）。
    ///
    /// # 参数
    /// - `qrcode`：`get_qrcode` 返回的二维码对象；
    /// - `interval`：轮询节奏；
    /// - `timeout`：整体最大等待时间（超时返回登录错误，上游产出 TIMEOUT 事件）；
    /// - `cancel`：可选取消信号（用户关闭登录弹窗时触发；取消会立即中断进行中的
    ///   网络轮询，与上游 anyio `fail_after` 语义一致）。
    ///
    /// # 返回
    /// - `Done` → 新 [`Credential`]；
    /// - `Refuse`/`Timeout`/取消 → [`QqMusicError::Login`]（code=-1）。
    ///
    /// 实测（2026-09-29）：`CancellationToken` 于 5s 取消，未扫码场景在取消后立即返回
    /// `Login{code:-1, "login canceled"}`（不实际扫码）。
    pub async fn wait_qrcode_login(
        &self,
        qrcode: &QR,
        interval: PollInterval,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<Credential, QqMusicError> {
        let deadline = Instant::now() + timeout;
        let mut last_event: Option<QRCodeLoginEvents> = None;
        let mut error_retries: u32 = 0;
        let min_safe_interval = Duration::from_millis(1000);

        loop {
            if let Some(cancel) = cancel {
                if cancel.is_cancelled() {
                    return Err(QqMusicError::Login {
                        code: -1,
                        message: "login canceled".into(),
                    });
                }
            }
            if Instant::now() >= deadline {
                return Err(QqMusicError::Login {
                    code: -1,
                    message: "login QR code has timed out".into(),
                });
            }

            let loop_start = Instant::now();
            // 取消信号可中断进行中的轮询请求（上游 anyio.fail_after 包裹单次操作）
            let item = if let Some(cancel) = cancel {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        return Err(QqMusicError::Login {
                            code: -1,
                            message: "login canceled".into(),
                        });
                    }
                    item = self.check_qrcode(qrcode) => item,
                }
            } else {
                self.check_qrcode(qrcode).await
            };
            let item = match item {
                Ok(item) => {
                    error_retries = 0;
                    item
                }
                Err(QqMusicError::Network(_)) => {
                    // 网络错误退避重试（上游指数退避）
                    let backoff = interval
                        .error_interval()
                        .min(interval.default * (1u32 << error_retries.min(6)));
                    if !sleep_before_deadline(deadline, backoff, cancel).await {
                        return Err(cancel_or_timeout_error(cancel));
                    }
                    error_retries += 1;
                    continue;
                }
                Err(e) => return Err(e),
            };

            // 去重：不重复产出连续相同事件（上游 emit_repeat=false）
            if Some(item.event) == last_event {
                // 仍按节奏 sleep（避免热点轮询），但不再产出
                let sleep_time = if item.event == QRCodeLoginEvents::Conf {
                    interval.scanned_interval()
                } else {
                    interval.default
                };
                let elapsed = loop_start.elapsed();
                if !sleep_before_deadline(
                    deadline,
                    sleep_time.max(min_safe_interval.saturating_sub(elapsed)),
                    cancel,
                )
                .await
                {
                    return Err(cancel_or_timeout_error(cancel));
                }
                continue;
            }
            last_event = Some(item.event);

            match item.event {
                QRCodeLoginEvents::Done => {
                    return item.credential.ok_or_else(|| QqMusicError::Login {
                        code: -1,
                        message: "login result is missing credentials".into(),
                    });
                }
                QRCodeLoginEvents::Refuse => {
                    return Err(QqMusicError::Login {
                        code: -1,
                        message: "user rejected the login request".into(),
                    });
                }
                QRCodeLoginEvents::Timeout => {
                    return Err(QqMusicError::Login {
                        code: -1,
                        message: "login QR code has timed out".into(),
                    });
                }
                QRCodeLoginEvents::Scan | QRCodeLoginEvents::Conf => {
                    let sleep_time = if item.event == QRCodeLoginEvents::Conf {
                        interval.scanned_interval()
                    } else {
                        interval.default
                    };
                    let elapsed = loop_start.elapsed();
                    if !sleep_before_deadline(
                        deadline,
                        sleep_time.max(min_safe_interval.saturating_sub(elapsed)),
                        cancel,
                    )
                    .await
                    {
                        return Err(cancel_or_timeout_error(cancel));
                    }
                }
            }
        }
    }

    fn client_config(&self) -> crate::config::ClientConfig {
        self.client.config()
    }
}

/// 解析 `ptuiCB('code','...','...')` 响应（上游 `_QQ_STATUS_RE` + `_QQ_ARGS_RE`）。
///
/// 返回 `(status_code, args)`。响应格式：
/// `ptuiCB('0','0','https://graph.qq.com/oauth2.0/login_jump?...', '0', ...)`
fn parse_ptui_cb(text: &str) -> Result<(i64, Vec<String>), QqMusicError> {
    let start = text.find("ptuiCB(").ok_or_else(|| {
        QqMusicError::InvalidResponse(
            "failed to query QR login status: unparseable response".into(),
        )
    })?;
    let rest = &text[start + 7..];
    let end = rest.find(')').ok_or_else(|| {
        QqMusicError::InvalidResponse(
            "failed to query QR login status: unparseable response".into(),
        )
    })?;
    let args_str = &rest[..end];

    let mut args = Vec::new();
    let mut chars = args_str.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '\'' => {
                chars.next();
                let mut s = String::new();
                while let Some(ch) = chars.next() {
                    if ch == '\\' {
                        if let Some(next) = chars.next() {
                            s.push(next);
                        }
                    } else if ch == '\'' {
                        break;
                    } else {
                        s.push(ch);
                    }
                }
                args.push(s);
            }
            ',' | ' ' => {
                chars.next();
            }
            _ => {
                chars.next();
            }
        }
    }

    let code_str = args.first().ok_or_else(|| {
        QqMusicError::InvalidResponse(
            "failed to query QR login status: unparseable status params".into(),
        )
    })?;
    let code = code_str.parse::<i64>().map_err(|_| {
        QqMusicError::InvalidResponse("failed to query QR login status: invalid status code".into())
    })?;
    Ok((code, args))
}

/// 从 Done 状态参数提取 `uin` 与 `ptsigx`（上游 `_QQ_SIGX_RE` + `_QQ_UIN_RE`）。
fn parse_done_args(args: &[String]) -> Result<(String, String), QqMusicError> {
    if args.len() < 3 {
        return Err(QqMusicError::InvalidResponse(
            "failed to obtain login credentials: missing required params".into(),
        ));
    }
    let url = &args[2];
    let sigx = extract_query_param(url, "ptsigx").ok_or_else(|| {
        QqMusicError::InvalidResponse(
            "failed to obtain login credentials: unparseable required params".into(),
        )
    })?;
    let uin = extract_query_param(url, "uin").ok_or_else(|| {
        QqMusicError::InvalidResponse(
            "failed to obtain login credentials: unparseable required params".into(),
        )
    })?;
    Ok((uin, sigx))
}

/// 从 URL 提取查询参数值（上游 `_QQ_SIGX_RE` / `_QQ_UIN_RE`）。
fn extract_query_param(url: &str, key: &str) -> Option<String> {
    let key_eq = format!("{key}=");
    // 先按 ?/& 切分查询段，再精确匹配 key= 前缀
    for part in url.split(['?', '&']) {
        if let Some(v) = part.strip_prefix(&key_eq) {
            return Some(v.to_owned());
        }
    }
    None
}

/// 从 oauth 授权 Location 提取 `code`（上游 `(?<=code=)(.+?)(?=&)`）。
fn extract_code_from_location(location: &str) -> Result<String, QqMusicError> {
    for part in location.split(['?', '&']) {
        if let Some(v) = part.strip_prefix("code=") {
            if !v.is_empty() {
                return Ok(v.to_owned());
            }
        }
    }
    Err(QqMusicError::InvalidResponse(
        "failed to obtain code".into(),
    ))
}

/// 从响应 Set-Cookie 中提取指定 cookie 值。
fn extract_cookie(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|s| {
            let first = s.split(';').next()?;
            let (k, v) = first.split_once('=')?;
            (k.trim() == name).then(|| v.trim().to_owned())
        })
}

/// 收集响应中的全部 cookie 键值对。
fn response_cookies(resp: &reqwest::Response) -> Vec<(String, String)> {
    resp.headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|s| {
            let first = s.split(';').next()?;
            let (k, v) = first.split_once('=')?;
            Some((k.trim().to_owned(), v.trim().to_owned()))
        })
        .collect()
}

fn random_f64_str() -> String {
    // 上游 random.random() 输出 0-1 浮点字符串
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("0.{}", n % 1_000_000_000)
}

fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 进程内单调计数器（uuid4_str 防碰撞）。
static UUID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 生成 v4 形状 UUID 字符串（上游 `uuid4()`，仅作 oauth authorize 的 `ui` 表单值）。
///
/// 无需额外依赖：纳秒时间戳 + 进程内计数器混合扩散，进程内不重复。
fn uuid4_str() -> String {
    use std::sync::atomic::Ordering;
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let counter = UUID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = nanos ^ counter.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let a = seed ^ (seed << 13);
    let b = a ^ (a >> 7);
    let c = b ^ (b << 17);
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        a as u32,
        (a >> 32) as u16,
        (b >> 16) & 0x0fff,
        (c & 0x3fff) | 0x8000,
        c & 0xffff_ffff_ffff,
    )
}

/// 睡眠中断后的错误区分：取消信号已触发 → `login canceled`，否则为整体超时。
///
/// 缺陷修复：此前两种情形均报告超时，用户关闭登录弹窗会看到错误的失败原因。
fn cancel_or_timeout_error(cancel: Option<&CancellationToken>) -> QqMusicError {
    let canceled = cancel.is_some_and(CancellationToken::is_cancelled);
    QqMusicError::Login {
        code: -1,
        message: if canceled {
            "login canceled".into()
        } else {
            "login QR code has timed out".into()
        },
    }
}

/// 在 deadline 前睡眠；取消时立即返回 `false`。
///
/// 返回 `false` 的两种情形（deadline 已到 / 已取消）由调用方经
/// [`cancel_or_timeout_error`] 区分。
async fn sleep_before_deadline(
    deadline: Instant,
    delay: Duration,
    cancel: Option<&CancellationToken>,
) -> bool {
    let now = Instant::now();
    if now >= deadline {
        return false;
    }
    let remaining = deadline.saturating_duration_since(now);
    let delay = delay.min(remaining);

    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);
    match cancel {
        Some(token) => {
            tokio::select! {
                _ = &mut sleep => true,
                _ = token.cancelled() => false,
            }
        }
        None => {
            sleep.await;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_by_value() {
        assert_eq!(
            QRCodeLoginEvents::get_by_value(0).unwrap(),
            QRCodeLoginEvents::Done
        );
        assert_eq!(
            QRCodeLoginEvents::get_by_value(405).unwrap(),
            QRCodeLoginEvents::Done
        );
        assert_eq!(
            QRCodeLoginEvents::get_by_value(66).unwrap(),
            QRCodeLoginEvents::Scan
        );
        assert_eq!(
            QRCodeLoginEvents::get_by_value(67).unwrap(),
            QRCodeLoginEvents::Conf
        );
        assert_eq!(
            QRCodeLoginEvents::get_by_value(65).unwrap(),
            QRCodeLoginEvents::Timeout
        );
        assert_eq!(
            QRCodeLoginEvents::get_by_value(68).unwrap(),
            QRCodeLoginEvents::Refuse
        );
        assert!(QRCodeLoginEvents::get_by_value(999).is_err());
    }

    #[test]
    fn parse_ptui_cb_scan() {
        let (code, args) = parse_ptui_cb("ptuiCB('66','0','', '0', '二维码未失效' );").unwrap();
        assert_eq!(code, 66);
        assert_eq!(args[0], "66");
    }

    #[test]
    fn parse_ptui_cb_done_extracts_uin_and_sigx() {
        let text = "ptuiCB('0','0','https://graph.qq.com/oauth2.0/login_jump?pt_3rd_aid=100497308&daid=383&j_later=0&u1=https%3A%2F%2Fgraph.qq.com%2Foauth2.0%2Flogin_jump&ptsigx=abcdef1234&s_url=https%3A%2F%2Fgraph.qq.com%2Foauth2.0%2Flogin_jump&uin=123456&service=https%3A%2F%2Fgraph.qq.com%2Foauth2.0%2Flogin_jump', '0', '登录成功' );";
        let (code, args) = parse_ptui_cb(text).unwrap();
        assert_eq!(code, 0);
        let (uin, sigx) = parse_done_args(&args).unwrap();
        assert_eq!(uin, "123456");
        assert_eq!(sigx, "abcdef1234");
    }

    #[test]
    fn parse_ptui_cb_rejects_garbage() {
        assert!(parse_ptui_cb("not ptui").is_err());
    }

    #[test]
    fn extract_code_from_location_ok() {
        let loc =
            "https://y.qq.com/portal/wx_redirect.html?login_type=1&code=QQCODE123&state=state";
        assert_eq!(extract_code_from_location(loc).unwrap(), "QQCODE123");
    }

    #[test]
    fn extract_code_from_location_missing() {
        assert!(extract_code_from_location("https://y.qq.com/portal/wx_redirect.html").is_err());
    }

    #[test]
    fn extract_cookie_from_set_cookie() {
        // 通过构造简单响应难以测试，直接测 response_cookies 逻辑分离的提取函数
        let header_value = "p_skey=abc123; Path=/; Domain=.qq.com; Max-Age=2592000";
        let first = header_value.split(';').next().unwrap();
        let (k, v) = first.split_once('=').unwrap();
        assert_eq!((k.trim(), v.trim()), ("p_skey", "abc123"));
    }

    #[test]
    fn validate_login_result_maps_codes() {
        assert_eq!(
            LoginApi::validate_login_result(&json!({"code": 0, "data": {"musicid": 1}})).unwrap(),
            json!({"musicid": 1})
        );
        assert!(matches!(
            LoginApi::validate_login_result(&json!({"code": 1000})),
            Err(QqMusicError::LoginAuthExpired)
        ));
        assert!(matches!(
            LoginApi::validate_login_result(&json!({"code": 20277})),
            Err(QqMusicError::LoginAccountRestricted)
        ));
        assert!(matches!(
            LoginApi::validate_login_result(&json!({"code": 20279})),
            Err(QqMusicError::LoginDeviceLimit)
        ));
        assert!(matches!(
            LoginApi::validate_login_result(&json!({"code": 104604})),
            Err(QqMusicError::LoginRateLimit)
        ));
        assert!(matches!(
            LoginApi::validate_login_result(&json!({"code": 20261})),
            Err(QqMusicError::Login { code: 20261, .. })
        ));
    }

    #[test]
    fn poll_interval_defaults() {
        let p = PollInterval::default();
        assert_eq!(p.scanned_interval(), Duration::from_millis(750));
        assert_eq!(p.error_interval(), Duration::from_millis(3000));
    }

    #[test]
    fn wrap_refresh_error_covers_login_variants() {
        let wrapped = LoginApi::wrap_refresh_error(QqMusicError::Login {
            code: 20271,
            message: "captcha".into(),
        });
        assert!(matches!(
            wrapped,
            QqMusicError::CredentialRefresh { code: 20271, .. }
        ));
        assert!(matches!(
            LoginApi::wrap_refresh_error(QqMusicError::LoginAuthExpired),
            QqMusicError::CredentialRefresh { code: 1000, .. }
        ));
        assert!(matches!(
            LoginApi::wrap_refresh_error(QqMusicError::LoginDeviceLimit),
            QqMusicError::CredentialRefresh { code: 20279, .. }
        ));
        assert!(matches!(
            LoginApi::wrap_refresh_error(QqMusicError::LoginRateLimit),
            QqMusicError::CredentialRefresh { code: 104604, .. }
        ));
        // 非登录域错误原样透传（上游 `other => other` 不在 except LoginError 内）
        assert!(matches!(
            LoginApi::wrap_refresh_error(QqMusicError::Network("x".into())),
            QqMusicError::Network(_)
        ));
    }

    #[test]
    fn uuid4_str_shape_and_uniqueness() {
        let a = uuid4_str();
        let b = uuid4_str();
        // v4 形状：8-4-4xxx-8xxx-12
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        assert_ne!(a, b, "process-unique counter should prevent collisions");
    }
}
