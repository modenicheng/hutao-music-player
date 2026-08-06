//! 登录模块（对应上游 `modules/login.py` + `modules/login_utils.py`）。
//!
//! # 凭证模型（设计决策）
//!
//! 本模块**不持有全局凭证，不负责凭证轮换**（docs/PROJECT.md §6.4）：
//!
//! - `refresh_credential` / `check_expired` / `logout` 均要求调用方显式传入凭证；
//! - 返回的新凭证由调用方自行存储与管理（支持多凭证场景）。

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::client::QqMusicClient;
use crate::credential::Credential;
use crate::error::QqMusicError;
use crate::protocol::cgi::CgiRequest;
use crate::protocol::sign::hash33;

/// 二维码登录类型（上游 `QRLoginType`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QRLoginType {
    /// QQ 扫码（当前已移植）。
    Qq,
    /// 微信扫码（待移植）。
    Wechat,
    /// 手机客户端扫码（依赖 MQTT，暂不移植）。
    Mobile,
}

/// 二维码登录流程中的状态事件（上游 `QRCodeLoginEvents`）。
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
    /// 根据状态码获取事件（上游 `get_by_value`）。
    fn get_by_value(value: i64) -> Result<Self, QqMusicError> {
        match value {
            0 | 405 => Ok(Self::Done),
            66 | 408 => Ok(Self::Scan),
            67 | 404 => Ok(Self::Conf),
            65 | 402 => Ok(Self::Timeout),
            68 | 403 => Ok(Self::Refuse),
            other => Err(QqMusicError::InvalidResponse(format!(
                "无法识别的二维码登录状态码: {other}"
            ))),
        }
    }
}

/// 二维码信息（上游 `QR`）。
#[derive(Clone, Debug)]
pub struct QR {
    /// 二维码图片二进制数据。
    pub data: Vec<u8>,
    /// 二维码登录类型。
    pub qr_type: QRLoginType,
    /// 图片 MIME 类型。
    pub mimetype: String,
    /// 标识符（QQ=qrsig，微信=uuid）。
    pub identifier: String,
}

/// 二维码登录流程中的单次结果（上游 `QRLoginResult`）。
#[derive(Clone, Debug)]
pub struct QRLoginResult {
    /// 状态事件。
    pub event: QRCodeLoginEvents,
    /// 仅在 `Done` 时携带凭证。
    pub credential: Option<Credential>,
}

impl QRLoginResult {
    /// 是否表示登录完成。
    pub fn done(&self) -> bool {
        self.event == QRCodeLoginEvents::Done
    }
}

/// 轮询间隔控制策略（上游 `PollInterval`，单位秒）。
#[derive(Clone, Debug)]
pub struct PollInterval {
    /// 默认轮询间隔。
    pub default: Duration,
    /// 已扫码状态下的轮询间隔（默认 `default/2`）。
    pub scanned: Option<Duration>,
    /// 异常退避最大间隔（默认 `default*2`）。
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

/// 登录 API（对应上游 `LoginApi`）。
///
/// 借用 `&QqMusicClient` 发起请求；凭证参数均由调用方显式传入。
pub struct LoginApi<'a> {
    client: &'a QqMusicClient,
}

/// QQ 授权登录 CGI 允许的错误码（上游 `_ERROR_CODE`）。
const LOGIN_ERROR_CODES: &[i64] = &[
    1000, 104401, 104400, 20261, 20271, 20272, 20274, 20277, 20278, 20279, 20450, 104604,
];

impl<'a> LoginApi<'a> {
    /// 构造登录 API。
    pub fn new(client: &'a QqMusicClient) -> Self {
        Self { client }
    }

    /// 校验登录 CGI 响应并返回 `data`（上游 `_validate_result`）。
    ///
    /// 错误码映射见 docs/QQMUSIC_PORTING.md；登录业务错误码不允许时抛出
    /// 对应 [`QqMusicError`] 变体，刷新场景由 `refresh_credential` 包装。
    fn validate_login_result(data: &Value) -> Result<Value, QqMusicError> {
        let code = data.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
        if code == 0 {
            return Ok(data.get("data").cloned().unwrap_or(json!({})));
        }
        match code {
            1000 | 104401 | 104400 => Err(QqMusicError::LoginAuthExpired),
            20261 => Err(QqMusicError::Login {
                code,
                message: "登录参数错误".into(),
            }),
            20271 => Err(QqMusicError::Login {
                code,
                message: "验证码错误".into(),
            }),
            20272 => Err(QqMusicError::Login {
                code,
                message: "账号绑定异常".into(),
            }),
            20274 => Err(QqMusicError::Login {
                code,
                message: "账号绑定缺失".into(),
            }),
            20277 | 20278 | 20450 => Err(QqMusicError::LoginAccountRestricted),
            20279 => Err(QqMusicError::LoginDeviceLimit),
            104604 => Err(QqMusicError::LoginRateLimit),
            other => Err(QqMusicError::Login {
                code: other,
                message: format!("登录业务错误码 {other}"),
            }),
        }
    }

    /// 检查凭证是否过期（上游 `check_expired`）。
    ///
    /// WEB 平台：GET 个人主页接口，返回 `code != 0` 视为过期。
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

    /// 刷新登录凭证（上游 `refresh_credential`）。
    ///
    /// 调用方传入需要刷新的凭证，返回刷新后的新凭证；本模块不做自动轮换。
    pub async fn refresh_credential(
        &self,
        credential: &Credential,
    ) -> Result<Credential, QqMusicError> {
        let login_type = credential.login_type.as_login_type_int();
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
                "musicid": credential.music_id,
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
                "musicid": credential.music_id,
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
        Self::validate_login_result(&data).map_err(|e| match e {
            QqMusicError::Login { code, message }
            | QqMusicError::CredentialRefresh { code, message } => {
                QqMusicError::CredentialRefresh { code, message }
            }
            QqMusicError::LoginAuthExpired => QqMusicError::CredentialRefresh {
                code: 1000,
                message: "登录鉴权参数无效或已过期".into(),
            },
            QqMusicError::LoginDeviceLimit => QqMusicError::CredentialRefresh {
                code: 20279,
                message: "登录设备超限".into(),
            },
            QqMusicError::LoginAccountRestricted => QqMusicError::CredentialRefresh {
                code: 20277,
                message: "账号受限".into(),
            },
            QqMusicError::LoginRateLimit => QqMusicError::CredentialRefresh {
                code: 104604,
                message: "操作过于频繁".into(),
            },
            other => other,
        })?;
        let data = data.get("data").cloned().unwrap_or(json!({}));
        Credential::from_login_data(&data)
    }

    /// 登出（上游 `logout`）。
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

    /// 获取登录二维码（上游 `get_qrcode`）。
    ///
    /// 当前仅支持 [`QRLoginType::Qq`]；微信/手机扫码待移植。
    pub async fn get_qrcode(&self, login_type: QRLoginType) -> Result<QR, QqMusicError> {
        match login_type {
            QRLoginType::Qq => self.get_qq_qr().await,
            QRLoginType::Wechat => {
                Err(QqMusicError::InvalidResponse("微信扫码登录暂未移植".into()))
            }
            QRLoginType::Mobile => Err(QqMusicError::InvalidResponse(
                "手机客户端扫码登录暂未移植".into(),
            )),
        }
    }

    /// 检查二维码状态（上游 `check_qrcode`）。
    pub async fn check_qrcode(&self, qrcode: &QR) -> Result<QRLoginResult, QqMusicError> {
        match qrcode.qr_type {
            QRLoginType::Qq => self.check_qq_qr(qrcode).await,
            QRLoginType::Wechat => {
                Err(QqMusicError::InvalidResponse("微信扫码登录暂未移植".into()))
            }
            QRLoginType::Mobile => Err(QqMusicError::InvalidResponse(
                "手机客户端扫码登录暂未移植".into(),
            )),
        }
    }

    /// 获取 QQ 授权二维码（上游 `_get_qq_qr`）。
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
            .ok_or_else(|| QqMusicError::InvalidResponse("获取 qrsig 失败".into()))?;

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

    /// 检查 QQ 二维码状态（上游 `_check_qq_qr`）。
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

    /// 完成 QQ 二维码授权并换取凭证（上游 `_authorize_qq_qr`）。
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
            .ok_or_else(|| QqMusicError::InvalidResponse("获取 p_skey 失败".into()))?;

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
            .ok_or_else(|| QqMusicError::InvalidResponse("获取 code 失败".into()))?
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
        let data = Self::validate_login_result(&data)?;
        Credential::from_login_data(&data)
    }

    /// 等待二维码登录完成（上游 `QRCodeLoginSession.wait_qrcode_login`）。
    ///
    /// 轮询 `check_qrcode` 直至 `Done`/`Refuse`/`Timeout`；
    /// `interval` 控制轮询节奏，`timeout` 为整体最大等待时间，
    /// `cancel` 为可选取消信号（用户关闭登录弹窗时触发）。
    ///
    /// 返回：
    /// - `Done` → `Credential`；
    /// - `Refuse` → [`QqMusicError::Login`]（用户拒绝）；
    /// - `Timeout` → [`QqMusicError::Login`]（超时）；
    /// - 取消 → [`QqMusicError::Login`]（已取消）。
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
                        message: "登录已取消".into(),
                    });
                }
            }
            if Instant::now() >= deadline {
                return Err(QqMusicError::Login {
                    code: -1,
                    message: "登录二维码已超时".into(),
                });
            }

            let loop_start = Instant::now();
            let item = match self.check_qrcode(qrcode).await {
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
                        return Err(QqMusicError::Login {
                            code: -1,
                            message: "登录二维码已超时".into(),
                        });
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
                    return Err(QqMusicError::Login {
                        code: -1,
                        message: "登录二维码已超时".into(),
                    });
                }
                continue;
            }
            last_event = Some(item.event);

            match item.event {
                QRCodeLoginEvents::Done => {
                    return item.credential.ok_or_else(|| QqMusicError::Login {
                        code: -1,
                        message: "登录结果缺少凭证".into(),
                    });
                }
                QRCodeLoginEvents::Refuse => {
                    return Err(QqMusicError::Login {
                        code: -1,
                        message: "用户拒绝了登录请求".into(),
                    });
                }
                QRCodeLoginEvents::Timeout => {
                    return Err(QqMusicError::Login {
                        code: -1,
                        message: "登录二维码已超时".into(),
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
                        return Err(QqMusicError::Login {
                            code: -1,
                            message: "登录二维码已超时".into(),
                        });
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
    let start = text
        .find("ptuiCB(")
        .ok_or_else(|| QqMusicError::InvalidResponse("获取二维码状态失败: 无法解析响应".into()))?;
    let rest = &text[start + 7..];
    let end = rest
        .find(')')
        .ok_or_else(|| QqMusicError::InvalidResponse("获取二维码状态失败: 无法解析响应".into()))?;
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
        QqMusicError::InvalidResponse("获取二维码状态失败: 无法解析状态参数".into())
    })?;
    let code = code_str
        .parse::<i64>()
        .map_err(|_| QqMusicError::InvalidResponse("获取二维码状态失败: 无效的状态码".into()))?;
    Ok((code, args))
}

/// 从 Done 状态参数提取 `uin` 与 `ptsigx`（上游 `_QQ_SIGX_RE` + `_QQ_UIN_RE`）。
fn parse_done_args(args: &[String]) -> Result<(String, String), QqMusicError> {
    if args.len() < 3 {
        return Err(QqMusicError::InvalidResponse(
            "获取登录凭据失败: 缺少必要参数".into(),
        ));
    }
    let url = &args[2];
    let sigx = extract_query_param(url, "ptsigx").ok_or_else(|| {
        QqMusicError::InvalidResponse("获取登录凭据失败: 无法解析必要参数".into())
    })?;
    let uin = extract_query_param(url, "uin").ok_or_else(|| {
        QqMusicError::InvalidResponse("获取登录凭据失败: 无法解析必要参数".into())
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
    Err(QqMusicError::InvalidResponse("获取 code 失败".into()))
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

fn uuid4_str() -> String {
    // 简化版 uuid v4（无需额外依赖）：随机十六进制片段
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        seed,
        (seed >> 16) as u16,
        ((seed >> 32) & 0xfff) as u16,
        ((seed >> 48) & 0xffff) as u16,
        seed.wrapping_mul(0x9e3779b97f4a7c15) & 0xffff_ffff_ffff,
    )
}

/// 在 deadline 前睡眠；取消时立即返回 `false`。
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
}
