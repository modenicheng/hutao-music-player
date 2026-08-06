# QQMusicApi 移植跟踪文档（QQMUSIC_PORTING.md）

> 本文档逐模块记录 Python 参考实现与 Rust 移植的对应关系、已移植/未移植接口、
> fixture、已知差异与 Live 测试结果（docs/PROJECT.md §24）。
> 移植过程中任何新增/删除的模块映射都必须在此登记。

## 参考实现

| 项目 | 值 |
| --- | --- |
| 仓库 | https://github.com/L-1124/QQMusicApi |
| 固定 commit | `108617ffe80abefec6358717b9f4d3677550db10`（2026 主线 `main`） |
| 许可证 | GPL-3.0-or-later（与 HMP 一致） |
| 语言/依赖 | Python 3.10+；niquests、pydantic、orjson、anyio、cryptography |

> 参考源通过 `scripts/fetch-python-ref.sh` 按需拉取到 `.deps/qqmusic-api-python/`
> （已被 .gitignore 排除），不通过 submodule 引入，避免双重提交/子模块指针/CI 成本
> （决策记录见 docs/PROJECT.md §5 讨论）。

## 模块映射

| Python 源文件 | Rust 目标模块 | 状态 | 备注 |
| --- | --- | --- | --- |
| `qqmusic_api/core/request.py` | `protocol/cgi.rs` | ✅ 已移植 | CgiRequest 描述符、错误码映射、批量信封解包 |
| `qqmusic_api/core/api_context.py` | `protocol/comm.rs` | ✅ 已移植 | comm 构造、Cookie 注入、UA |
| `qqmusic_api/core/versioning.py` | `protocol/comm.rs` | ✅ 已移植 | Platform、VersionProfile、g_tk |
| `qqmusic_api/algorithms/sign.py` | `protocol/sign.rs` | ✅ 已移植 | hash33、zzc_sign |
| `qqmusic_api/core/client.py` | `client.rs` | 🔶 部分 | musicu 请求入口；无全局凭证状态、无 Android 会话/限流 |
| `qqmusic_api/core/exceptions.py` | `error.rs` | ✅ 已移植 | 错误分类（§12 适配） |
| `qqmusic_api/models/request.py` | `credential.rs` | ✅ 已移植 | Credential（脱敏 Debug，无全局持有） |
| `qqmusic_api/modules/search.py` | （待移植） | ⬜ 未移植 | 阶段 A 验收后移植 |
| `qqmusic_api/modules/login.py` | （待移植） | ⬜ 未移植 | 阶段 B |
| `qqmusic_api/modules/song.py` | （待移植） | ⬜ 未移植 | 阶段 C |
| `qqmusic_api/modules/lyric.py` | （待移植） | ⬜ 未移植 | 阶段 C |
| `qqmusic_api/modules/songlist.py` | （待移植） | ⬜ 未移植 | 阶段 D |
| `qqmusic_api/utils/device.py` | （待移植） | ⬜ 未移植 | 仅 Android 平台需要 |
| `qqmusic_api/utils/qimei.py` | （待移植） | ⬜ 未移植 | 仅 Android 平台需要 |
| `qqmusic_api/utils/mqtt.py` | — | ⬜ 不移植 | HMP 非目标功能 |

## 已移植接口

### 基础请求层（阶段 A，docs/PROJECT.md §6.6）

- [x] `hash33(s, h=0)` → `crates/hmp-qqmusic-api::protocol::sign::hash33`
- [x] `zzc_sign(payload)` → `protocol::sign::zzc_sign`
- [x] `VersionPolicy.get_g_tk(credential)` → `protocol::comm::g_tk`
- [x] `Platform`（ANDROID/DESKTOP/WEB）→ `protocol::comm::Platform`
- [x] `VersionPolicy.build_comm(...)`（WEB 平台）→ `protocol::comm::build_web_comm`
- [x] `ApiContext.build_api_kwargs(...)` → `client::QqMusicClient::musicu_request`
- [x] `Client._unwrap_cgi_batch(...)` → `protocol::cgi::unwrap_cgi_batch`
- [x] `CgiRequest._parse_response(...)` → `protocol::cgi::CgiRequest::parse_response`
- [x] Cookie 注入（`uin`/`qqmusic_uin`/`qm_keyst`/`qqmusic_key`）
- [x] 日志脱敏（Cookie、music key 不落日志）

## 尚未移植

- 登录（QQ 二维码 / 微信扫码 / 微信换取登录态）—— 阶段 B
- 播放 URL 获取 —— 阶段 C
- 歌词 —— 阶段 C
- 歌单 / 专辑 / 歌手 / 每日推荐 —— 阶段 D
- 写操作（收藏、歌单管理）—— 阶段 E
- Android 平台会话（`ensure_session`/QIMEI/设备指纹）—— HMP 目标为 Linux 桌面，暂不移植

## Fixture

### 目录约定

- `crates/hmp-qqmusic-api/tests/fixtures/`：随 crate 发布的解析测试 fixture（离线、CI 默认运行）
- `fixtures/qqmusic/`：仓库级差分测试原始录制（Python/Rust 对比，本地运行）

### 现有 fixture

| 文件 | 来源 | 用途 |
| --- | --- | --- |
| `tests/fixtures/search/quick_song.json` | Live 录制（免登录 smartbox） | quick_search 解析测试 |
| `tests/fixtures/cgi/error_code.json` | （预留） | 错误码映射 |

### 搜索接口实测记录（2026-08-06）

- `music.search.SearchCgiService/DoSearchForQQMusicDesktop`：返回空列表（旧方法，已失效）；
- `music.search.SearchCgiService/DoSearchForQQMusicMobile`：需 Android 平台参数，WEB comm 下歌曲为空；
- `music.adaptor.SearchAdaptor/do_search_v2`（general_search）：需登录态，免登录下无歌曲数据；
- `c.y.qq.com/splcloud/fcgi-bin/smartbox_new.fcg`（quick_search）：**免登录可用**，返回歌曲/专辑/歌手/MV，
  阶段 A 采用此入口。

## 已知差异

| 项 | Python 参考 | Rust 移植 | 说明 |
| --- | --- | --- | --- |
| HTTP 客户端 | niquests（multiplexed、令牌桶限流） | reqwest | 限流由 HMP 应用层控制 |
| Android 平台 | 完整支持（QIMEI/设备会话） | 不移植 | HMP 面向 Linux 桌面 |
| 响应模型 | pydantic BaseModel | serde（DTO 起步允许 `serde_json::Value`） | 稳定后逐步强类型化 |
| 布尔参数 | `bool_to_int` 自动转换 | 显式 int 转换 | 保持可读性 |
| **凭证模型** | client 持有全局 `credential`，方法可选覆盖 | **无全局凭证状态** | 请求级传入；仅显式 `refresh_credential`；调用方管理多凭证 |

## 设计决策记录

### 凭证解耦（2026-08-06，docs/PROJECT.md §6.4）

- 客户端不持有全局凭证，无自动轮换/定时刷新；
- 需要登录态的请求由调用方传入 `Option<&Credential>`；
- 刷新仅通过显式接口 `refresh_credential(&Credential) -> Credential`（阶段 B 实现）；
- 调用方负责 keyring 存储与过期判断，客户端返回业务错误码供调用方决策。

## Live 测试

> 需要真实账号与网络，默认忽略；Live 测试不使用个人 Cookie 提交公共 CI。

```bash
cargo test --features live-tests -- --ignored
```

环境变量：`HMP_QQMUSIC_COOKIE`、`HMP_LIVE_TEST_TRACK_ID`（不写入仓库）。

## 上游变化记录

| 日期 | commit | 变化 | Rust 侧影响 |
| --- | --- | --- | --- |
| 2026-08-06 | `108617f` | 基线（首次移植） | — |
