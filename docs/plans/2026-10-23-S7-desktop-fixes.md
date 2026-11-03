# S7 计划：桌面两小修（右键菜单 picker 不可达 + 桌面请求读超时）

- 工作树：`D:\coding\hutao-music-player\.worktrees\s7-desktop-fixes`（分支 `fix/desktop-picker-timeout`，基线 main `68f8424`）
- 计划代理产出；实施前经独立审核迭代。**已按独立评审修订（2026-10-23）**：必改三项（PORTING.md:188 更正纳入本流任务与所有权；超时值归因改写——CoverGet 上游为 hmp_media `cdn_client()` connect 10s + 按次读 30s，15s 是 QQ API 客户端不覆盖封面，CLI 同源注释错误经协调者授权一并更正；主模式行坐标公式补分隔线 5px）与可选三项（S1 冲突论证修正、调用面补全、引用行号修正）全部采纳。
- 两项修复**各自独立、可单独裁剪**（裁剪任一项不影响另一项的文件与测试）。

---

## ① 目标与范围

### 项 A：右键菜单「加入播放列表…」二级 picker 不可达修复（S6 验收缺陷 #1，高严重度）

- **缺陷**：`crates/hmp-desktop/ui/track-table.slint:301` 与 `ui/queue-drawer.slint:365` 的
  `row-menu := PopupWindow` 未设 `close-policy`，slint 1.17.1 默认 `close-on-click` 使
  **弹层内任意完整点击（press+release）即关闭菜单**——主菜单点「加入播放列表…」后弹层随即
  被框架关闭，二级歌单选择面板（picker）不可达，「加入播放列表」功能在桌面端完全不可用
  （S6 verify/REPORT.md 缺陷 #1 证据链：press/release 单独弹层存活、完整 click 关闭、点禁用行
  同样关闭——纯框架策略行为，非业务回调驱动）。
- **目标**：两处宿主 PopupWindow 显式 `close-policy: close-on-click-outside;`，picker 可达；
  点外关闭、ESC 关闭、选择动作后整体关闭（`dismissed → close()` 显式路径）三语义不回归；
  补链路级集成测试（新测试文件，见 §⑤——testing backend 进程级单例，仓库惯例一文件一
  `#[test]`，不能并入既有 `context_menu.rs` 的测试函数）。
- **不在范围**：`quality-badge.slint:41` 的 PopupWindow（每个可选行都显式 `quality-pop.close()`，
  默认策略下的框架关闭只是冗余无害行为，功能不受影响——观察项，见 §⑦）。

### 项 B：桌面端请求读超时（S4 审计登记、明确归本微流的缺口）

- **缺口**：`crates/hmp-desktop/src/backend.rs:278-317` 的 `request()`（连接重试 4×250ms +
  写帧 + 读帧 + 解码）写读全程**无超时**——daemon 连上但挂死（应答之前卡住）时，桌面每一条
  命令/查询（播放、入队、歌单写入、搜索、封面、登录轮询、队列分页投影……）全部永久挂起，
  无错误、无降级、无日志。CLI 侧同型路径已有 30s 先例（S4 合并：`crates/hmp-cli/src/client.rs:31`
  `REQUEST_TIMEOUT` + `request_with_timeout`，超时返回 `CliError::Timeout`）。
- **目标**：`request()` 全程（连接重试+写+读+解码）套有界超时，超时归入新错误变体
  `BackendError::Timeout`，不误报 `StaleBackend`、不进 §22 可操作离线原因三元组、不触发
  Quit 重启 churn；订阅长连接（推送流）不经此路径、不受影响；补单测（临时端点静默服务器模式，
  镜像 CLI `request_times_out_when_daemon_silent`）。

---

## ② 现状与根因（证据）

### 项 A：slint 1.17.1 源码核实结论（Cargo.lock 锁定版本，registry 源码逐条核对）

**枚举定义**（`i-slint-common-1.17.1/enums.rs:617-626`，slint 属性名 kebab-case）：

```rust
enum PopupClosePolicy {
    CloseOnClick,        // close-on-click
    CloseOnClickOutside, // close-on-click-outside
    NoAutoClose,         // no-auto-close
}
```

**默认值 = `close-on-click`（value 0）**，证据链：

1. `i-slint-compiler-1.17.1/builtins.slint:2502-2505`：`in property <PopupClosePolicy> close-policy;`
   注释明言 "constexpr hardcoded in typeregister.rs"；
2. `i-slint-compiler-1.17.1/passes/lower_popups.rs:129-183`：close-policy 解析顺序为
   显式绑定 → 旧版 `close-on-click <bool>` 布尔映射（true→close-on-click / false→no-auto-close）
   → 基组件继承 → **兜底 `unwrap_or_else(|| EnumerationValue { value: 0, .. })`**；
   枚举 value 0 按声明序 = `CloseOnClick`；
3. `i-slint-core-1.17.1/window.rs:817-826` 关闭条件源码（逐 policy）：

```rust
match popup.close_policy {
    PopupClosePolicy::CloseOnClick => {
        let mouse_inside_popup = mouse_inside_popup();
        (mouse_inside_popup && released_event && self.had_popup_on_press.get())  // 弹层内完整点击即关
            || (!mouse_inside_popup && pressed_event)                            // 弹层外 press 即关
    }
    PopupClosePolicy::CloseOnClickOutside => !mouse_inside_popup() && pressed_event, // 仅弹层外 press 关
    PopupClosePolicy::NoAutoClose => false,
}
```

4. **ESC 行为与 policy 的关系**（`window.rs:1145-1172`）：`CloseOnClick` 与
   `CloseOnClickOutside` **均**在 ESC 按键时关闭顶层弹层（`close_on_escape` 条件为
   `policy == CloseOnClick || policy == CloseOnClickOutside`），`NoAutoClose` 不关。
   → 改为 close-on-click-outside **不改变 ESC 语义**（S6 报告判断「两策略 ESC 等价」属实）。
5. **显式 `close()` 与 policy 无关**（`close_top_popup`，`window.rs:1986`）：
   业务代码 `dismissed => row-menu.close()`（track-table.slint:308-310 / queue-drawer.slint:371-375）
   在任何 policy 下照常关闭。

**枚举语义表（核实后）**：

| policy | 弹层内完整点击 | 弹层外 press | ESC | 显式 close() |
|---|---|---|---|---|
| `close-on-click`（**默认**） | **关闭**（released && had_popup_on_press） | 关闭 | 关闭 | 有效 |
| `close-on-click-outside` | **不关闭** | 关闭 | 关闭 | 有效 |
| `no-auto-close` | 不关闭 | 不关闭 | 不关闭 | 唯一关闭途径 |

**根因**：两处宿主未写 close-policy → 落到默认 `close-on-click` → 弹层内任意完整点击关闭菜单。
`ui/context-menu.slint:202-204`「加入播放列表…」行的 `activated` 只置 `picker-mode = true`（面板内
切换，不关弹层——正确设计），框架随即替它关了。其余动作行因都显式 `dismiss()`（等价主动关闭），
症状被掩盖，唯 picker 受害。**次要根因（认知污染）**：`PORTING.md:188` 记载「默认
`CloseOnClickOutside` 策略」是**错的**（与源码相悖），`track-table.slint:299` 注释「点外/ESC 关闭走
CloseOnClickOutside 默认策略」沿用了同一错误认知——这解释了缺陷为何在多轮评审中漏网。本次修复
将同步更正两处代码注释**及 PORTING.md:188 的 ④ 子句**（评审必改 1：纳入本流任务表 T1c 与
§⑦ 所有权清单）。

**context-menu.slint 自关闭路径排查**（任务书要求）：面板内所有会「产生动作」的行都显式
`dismissed()`：播放(:174-177)、下一首播放(:182-185)、加入队列(:187-194)、从队列移除(:206-213)、
从歌单移除(:214-221)、查看歌手(:226-234)、查看专辑(:235-243)、**picker 内歌单行(:270-273，
`TrackMenu.add-to-playlist(p.id)` 后 `dismissed()`——选择歌单后菜单整体关闭正是期望行为**）。
不 dismissed 的仅两行：「加入播放列表…」(:199-205，切 picker)与 picker 返回行(:250-256，切回
主菜单)——设计上就该保持弹层打开。结论：**close-on-click-outside 与全部既有自关闭路径零冲突**；
它去掉的唯一框架关闭是「弹层内点击」这一条，恰好是缺陷本身。

**语法背书**：slint 自带 cosmic 风格 `combobox.slint:101` 即 `close-policy: close-on-click-outside;`
（裸枚举字面量；限定形式 `PopupClosePolicy.close-on-click-outside` 亦合法，popup3.slint 测试用例）。
属性为 Constexpr 可见性（typeregister.rs:562-564），字面量满足。关闭逻辑在 i-slint-core
`WindowInner`，与渲染后端无关——testing backend 的集成测试与真机（femtovg/winit）行为一致
（S6 已在 testing backend 实证缺陷，同一层代码路径）。

### 项 B：现状证据

- `backend.rs:278-317` `request()`：`connect()`（成功即出循环）→ `write_frame` → `read_frame` →
  `decode_frame`，全程无 `tokio::time::timeout`。`read_frame` 的 `read_exact` 在对端静默时
  永久挂起（无 EOF、无错误）。
- **调用面（单 choke point，全部经 `backend::request`；评审可选 2 补全）**：player_bridge.rs:114/130/
  427/617/653/971/1072/1186（播放/音量/收藏/歌词/评论/**CoverGet**）；**track_covers.rs:90 与
  online_covers.rs:60（CoverGet 的另两处消费方——库封面/在线封面）**；backend.rs:157/178（订阅
  首连的 Status 指纹探测）/230（restart_backend 的 Quit）/495（project_queue 的 QueueList 分页）；
  bridge.rs:164/242/315/374/431/943/953/988/1038（发现页/榜单/账号状态/扫码轮询/音质/搜索）；
  **bridge.rs:1075（LoginQrCancel）/1093（Logout）/1143、1201（GuessGet）**；menu_bridge.rs
  （PlaylistWrite，经 player_bridge::dispatch_command*）。词表内最慢合法路径 = CoverGet（daemon
  出网整图下载，见 §③ 超时值论证）。
- **订阅长连接不走 request()**：`spawn_state_subscription`（backend.rs:362-472）在自己 stream 上
  `write_frame(Subscribe)` + 循环 `read_frame`——推送流无「应答」语义，**不得**套请求超时（勿伤）。
  它与 request() 的交点仅两处：首连 `connect_or_spawn → verify_backend_generation` 内的 Status
  短连接探测（backend.rs:157、178）。
- **§22 语义**（docs/AUDIT.md §22）：`offline_reason_text`（backend.rs:218-225）只让
  StaleBackend / NoBackendBinary / SpawnTimeout 三类**环境性可操作**错误携带离线原因（toast 带
  修复指引）；普通 IO/协议断连沿用笼统提示。StaleBackend 误报的代价实锤见 §22：错误文案会把
  「重建二进制」指引给到并不需要的场景。
- **CLI 先例**（crates/hmp-cli/src/client.rs）：`REQUEST_TIMEOUT = 30s`（:28-31）；`request_with_timeout`
  （:81-104）`tokio::time::timeout` 包「编码+写+读+解码」全程，超时映射 `CliError::Timeout`（Display
  自带 `hmp quit` 恢复指引）；测试 `request_times_out_when_daemon_silent`（:180-211）：临时端点 +
  `IpcListener` 静默服务器（accept → 读走请求帧 → `std::future::pending()` 挂死）→ 断言
  `Timeout(150ms)` 且耗时 <1s。**注意**：client.rs:28-31 既有注释把最慢路径归因到「reqwest 自身
  15s 超时」是**错的**——15s 是 QQ API 客户端（hmp-qqmusic-api config.rs:31）的超时，CoverGet 封面
  下载走 daemon ContentService（content.rs:369）→ hmp_media `cdn_client()`（stream/source.rs:365-372：
  connect_timeout 10s + read_timeout 30s 按每次读计时，:371-372 注释明言 ContentService 复用）。
  **协调者已授权**：该 CLI 注释的同一归因错误随本流 B 项一并更正（仅注释一行）。
- **端点注入可行性**（transport.rs:225-246）：任意路径（含临时目录文件路径）在 Windows 上确定性
  映射 `\\.\pipe\hmp-<sanitized>` 管道名——CLI 测试的 tempdir 模式在桌面侧同样可用；
  `socket_path()` 虽支持 `HMP_IPC_ENDPOINT` 覆盖（server.rs:22-28），但 env 是进程全局、与并行测试
  互扰，**不采用**，改参数注入（见设计）。

---

## ③ 设计方案

### 项 A：显式 `close-policy: close-on-click-outside;`

**主方案**：两处宿主 PopupWindow 各加一行 + 更正注释：

- `ui/track-table.slint:301` 块（297-312）：
  - `row-menu := PopupWindow {` 下加 `close-policy: close-on-click-outside;`
  - 注释 ：297-299 中「点外/ESC 关闭走 CloseOnClickOutside 默认策略」改为如实描述：
    「弹层内点击不关（picker 二级面板依赖此语义，slint 默认 close-on-click 会连弹层内点击一起关，
    S6 缺陷 #1）；点外 press / ESC / dismissed 显式 close 三路关闭」
- `ui/queue-drawer.slint:365` 块（362-376）：同一行 + 注释「同 track-table」处同步更正。
- `ui/context-menu.slint`：**不动**（面板内部无自关闭路径依赖默认 policy；dismissed/close 显式
  路径完整）。改动面最小化也压低与并行流 S1（同样可能改 track-table.slint）的合并冲突。

**行为变化核对**（相对现状，全部为期望方向）：

| 交互 | 现状（默认 close-on-click） | 修复后 | 评价 |
|---|---|---|---|
| 点「加入播放列表…」 | 弹层被框架关闭，picker 不可达 | 弹层存活，面板切 picker | 修复目标 |
| picker 内点歌单行 | （不可达） | `add-to-playlist` + `dismissed→close()` 整体关闭 | 期望行为，显式路径不受 policy 影响 |
| 点其余动作行 | activated + 框架关闭 | activated + 显式 dismissed 关闭 | 等价（关闭时机从 release 略延至回调内，不可感知） |
| 弹层外 press | 关闭 | 关闭（同一条件 `!inside && pressed`） | 不变 |
| ESC | 关闭 | 关闭（两 policy 同门） | 不变 |
| 弹层内空白处（非行）完整点击 | 关闭 | 不关（需点行/点外/ESC） | 轻微 UX 差异，菜单惯例内 |
| 弹层内 press 拖到弹层外 release | 关（release 时 !inside… 注：默认策略此况实为 inside-press 路径，release 在外不满足 inside 条件 → 不关；两 policy 实际均不关此况） | 不关 | 无实际差异 |

（末行如实记录：核对 window.rs 条件，inside-press + outside-release 在**两种** policy 下都不触发
框架关闭，非本修复引入的变化。）

**备选方案（否决理由）**：

- **Alt A：`no-auto-close` + 全手动关闭**（自监听点外/ESC）：丢失内建点外与 ESC 语义，需自写
  覆盖层 TouchArea——PORTING.md 已警告覆盖 TouchArea 会截断行 hover 链；代码量大、坑多。
- **Alt B：旧版 `close-on-click: false` 布尔**：等价 no-auto-close（lower_popups.rs:118-127 映射），
  同 Alt A 被否；且属兼容遗留面，新代码不应采用。
- **Alt C：picker 拆独立二级 PopupWindow**（关一级再开二级）：重构面大、嵌套弹层定位/锚定复杂，
  违背「仅面板内切换」的既有设计（context-menu.slint:126-128），否决。

**风险**：见 §⑥。

### 项 B：request() 全程 30s 有界超时 + `BackendError::Timeout`

**主方案**（四部分，全在 backend.rs，不碰任何禁碰文件）：

1. **错误变体**：`BackendError::Timeout(Duration)`（Display 对齐 CLI 语义、中文可操作文案：
   `daemon {0:?} 未应答（可能已挂死）：请 hmp quit 终止后重试`）。`error::source` 走既有 `_ => None`；
   `offline_reason_text` 加**显式** arm `Timeout(_) => None` 并注释论证：挂死是瞬时/可自愈态
   （用户 quit 或 daemon 恢复后 2s 重连自愈），不是 §22 意义上的环境性可操作错误；订阅离线
   toast 沿用笼统「播放服务未连接」，恢复动作留在 Display → `report_command_result` 的
   `tracing::warn!("{e}: …")` 日志（player_bridge.rs 禁碰，其既有 `Err(e)` 泛化 arm 恰好兼容，
   无需任何改动）。
2. **超时包裹 + 可测性重构**（最小形变，CLI 同款）：

```rust
/// 单请求全程硬上限（连接重试 + 写 + 读 + 解码）：daemon 连上但挂死时
/// 报错而非永久挂起（AUDIT §16 #3；CLI REQUEST_TIMEOUT 同款同值）。
/// 30s 覆盖最慢合法路径：CoverGet 走 daemon ContentService → hmp_media
/// cdn_client（connect 10s + 按次读 30s；封面图小，单次读内完成），
/// QQ API 类请求上游另有 15s（hmp-qqmusic-api config.rs）。桌面超时只是
/// 放弃等待：daemon 仍会完成下载并落盘缓存，封面另有 COVER_RETRY
/// 退避兜底（player_bridge），重试即命中缓存。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn request(req: Request) -> Result<Response, BackendError> {
    request_with_timeout(&hmp_daemon::server::socket_path(), req, REQUEST_TIMEOUT).await
}

/// 带端点与预算的请求（单测注入临时端点/短超时；生产走 [`request`]）。
async fn request_with_timeout(endpoint: &Path, req: Request, timeout: Duration)
    -> Result<Response, BackendError>
{
    tokio::time::timeout(timeout, async {
        // 原 request() 主体原样迁入；重试循环内 connect() 改 IpcStream::connect(endpoint)
    }).await.map_err(|_| BackendError::Timeout(timeout))?
}
```

   连接重试（4×250ms）计入预算：最坏合法路径 1s 重试 + CoverGet ≈ 仍余 ~14s，成立。
   `connect()` 本体不动（obtain_stream/restart_backend 继续用）；仅重试循环内改直连 `endpoint`。
3. **`verify_backend_generation` 两处探测的超时归类**（防误报 StaleBackend——任务书关注点 ①）：
   - 首探测（backend.rs:157-162）加 arm 在 `Err(_)` 兜底**之前**：
     `Err(BackendError::Timeout(d)) => return Err(BackendError::Timeout(d))`。
     理由：现状保守放行（`Err(_) => Ok(stream)`）的依据是「断线由订阅循环既有重连路径处置」——
     但**挂死**的 daemon 让随后的 Subscribe 读**永远无声挂起**（订阅读无超时且不该有），保守放行
     把可检测的挂死变成不可检测的冻结；30s 超时远超任何瞬态抖动，恰是区分二者的信号。返回 Err
     → 订阅循环走既有离线 + 2s 退避重连（与 daemon 死亡同路径），恢复即自动重订。
     **不会引发 Quit churn**：Quit/重启只在 Status **成功应答且指纹不符**时触发，超时路径
     不触碰 `restart_backend`（§22 churn 防线不破）。
   - 重启后复核（backend.rs:178-186）加 arm 在 `_ => StaleBackend` **之前**：
     `Err(BackendError::Timeout(d)) => return Err(BackendError::Timeout(d))`，且**不写**
     `stale_backend_seen` 记账——超时不是「磁盘二进制陈旧」的证据；若落 `_` arm 会把挂死误报成
     StaleBackend（toast 指引用户重建二进制——错误补救），且记账会错误压制重建后的重启核对。
4. **不改动路径（明确记录）**：订阅长连接读写（勿伤，任务书关注点 ②）；`project_queue` 的
   `_ => return Vec::new()`（超时归入空投影，与既有 IO 错误降级同语义）；`restart_backend`
   的 Quit（受 30s 上界约束，最坏 30s + 退出轮询 12s ≈ 42s 有界，可接受；CLI 对 quit 同样
   套 30s 是明确先例）。

**超时值论证（任务书关注点 ③，评审必改 2 已修正归因）**：30s，与 CLI 完全对齐。桌面
`request()` 承载与 CLI 相同的 Request 词表，最慢合法路径是 CoverGet——其上游为 daemon
ContentService::cover（content.rs:369）→ hmp_media `cdn_client()`（source.rs:365-372：
**connect_timeout 10s + read_timeout 30s 按每次读计时**；封面图小，主体传输在单次读窗口内
完成，故合法总耗时上界 ≈ 连接 10s + 单读 30s 的组合远小于无限），QQ API 类请求上游另有
15s（config.rs:31）。30s 仍可辩护的四条理由：① CLI 同值先例（一致运维口径）；② 封面图小、
cdn_client 各段超时内必失败或完成，30s 内合法请求几乎不触顶；③ 桌面超时只是放弃**等待**，
daemon handler 继续跑完并落盘缓存（下次 COVER_RETRY / 预取即命中）；④ 封面消费方自带
COVER_RETRY 指数退避（player_bridge.rs:1128-1133，2s-60s）。更短（如 10s）会在慢网封面下载上
误伤，更长无收益（UI 侧全部后台 spawn，不阻塞事件循环，超时只是把「永久挂起」变「有界报错」）。

**备选方案（否决理由）**：

- **Alt A：按请求种类分档预算**（控制命令 5s / 出网查询 30s）：单 choke point 简单性被破坏，
  词表演化要维护分类表；CLI 已选统一 30s，无用户可见收益，否决。
- **Alt B：超时映射为 `BackendError::Io(ErrorKind::TimedOut)`**：省一个变体，但 Display 无恢复
  指引、日志不可辨「挂死」与普通 IO 断连、后续想区分时要破坏性改类；CLI 已证明独立变体的
  表达力，否决。
- **Alt C：只包读、不包连接**：Windows 管道连接在实例忙时有内部退避预算（transport.rs 客户端
  busy 退避），但 connect 理论上仍可无限等（对端 accept 循环死亡等）；全程序即最安全且实现
  更短，否决 C。
- **测试注入备选（否决）**：`HMP_IPC_ENDPOINT` env 覆盖注入——进程全局、与并行测试/其他用例
  互扰（desktop 测试进程内多线程跑），参数注入干净且零全局态。

---

## ④ 任务分解（文件清单 + 验收标准）

> 顺序：T1（项 A 代码）→ T2（项 A 测试，红→绿实证）→ T3（项 B 代码+单测）→ T4（全量验证）。
> 项 A/项 B 可独立裁剪：裁 A 则 T1/T2 整体去掉；裁 B 则 T3 去掉；二者无文件交集
> （A: ui/*.slint + tests/context_menu.rs；B: src/backend.rs）。

| # | 文件 | 改动 | 验收标准 |
|---|---|---|---|
| T1a | `crates/hmp-desktop/ui/track-table.slint` | :301 PopupWindow 加 `close-policy: close-on-click-outside;`；:297-299 注释更正 | 编译过；T2 新测试绿 |
| T1b | `crates/hmp-desktop/ui/queue-drawer.slint` | :365 同上一行；:362-363 注释更正 | 同上（同面板组件，一处策略两处声明） |
| T1c | `crates/hmp-desktop/PORTING.md` | :188 ④ 子句按源码事实改写：默认 **close-on-click**（lower_popups.rs 兜底 value 0；弹层内完整点击也关，含禁用行）；ESC/点外两策略同供（ESC 先于业务 esc-scope 收口）；**弹层内点击后不关的二级面板（picker 类）必须显式 close-on-click-outside**（记 S6 缺陷 #1 教训与本条 2026-10-04 错误记载的修正） | 并入 T1a 验收：与 A 项同提交，文案与 §② 证据链一致 |
| T2 | `crates/hmp-desktop/tests/context_menu_picker.rs`（**新文件**） | 集成测试 `context_menu_picker_reachable_and_closes_on_selection`（详见 §⑤；新文件而非扩 context_menu.rs：testing backend 进程级单例，仓库惯例一文件一 `#[test]`） | 修复前红（add-to-playlist 永不触发，先跑取证）/修复后绿；既有 `context_menu_snapshot_and_gates` 不回归 |
| T3a | `crates/hmp-desktop/src/backend.rs` | ① `BackendError::Timeout(Duration)` + Display/`offline_reason_text` 显式 arm；② `REQUEST_TIMEOUT` 常量（注释归因见 §③）+ `request` 拆 `request_with_timeout`（timeout 包全程，重试循环连 `endpoint`）；③ `verify_backend_generation` 两处探测加超时 arm（首探测传 Err、复核不记账不误报） | clippy -D warnings 零告警；既有单测全绿；新单测绿 |
| T3b | `crates/hmp-desktop/src/backend.rs`（tests mod） | 三个新单测（详见 §⑤）：超时路径 / 预算内完成路径 / Timeout 非可操作离线原因 | 全绿；超时测试断言 `Timeout(150ms)` 且耗时 <1s |
| T3c | `crates/hmp-cli/src/client.rs` | :28-31 注释归因更正（仅注释：15s→QQ API 客户端 config.rs:31；CoverGet 上游 = hmp_media cdn_client connect 10s + 按次读 30s）——协调者授权 | `cargo check -j 4 -p hmp-cli` 过；无行为改动 |
| T4 | 全仓验证 | `cargo fmt --all -- --check`；`cargo clippy --workspace --all-targets --all-features -- -D warnings`；`cargo test --workspace` 逐 crate `-j 1`（至少 hmp-desktop、hmp-cli 回归）；按 AGENTS 纪律重建 hmp + hmp-desktop 二进制、`hmp quit` 清旧 daemon 后真机冒烟（右键→picker→选歌单生效；常规命令不受影响） | 全绿 + 真机冒烟记录于实施汇报 |

提交切分（评审流程要求）：**A 项一个提交**（T1a/T1b/T1c + T2），**B 项一个提交**（T3a/T3b/T3c）；
PLAN.md 不随本流提交（协调者合并时迁 docs/plans/）。预估改动量：T1 ≈ 4 行 + PORTING 1 条；T2 ≈ +100 行；T3 ≈ +80 行（含测试与注释）。

---

## ⑤ 测试计划

### 项 A 集成测试（**新文件 `tests/context_menu_picker.rs`**，testing backend 单例 → 仓库惯例一文件一 `#[test]`；与既有 context_menu.rs 同模式）

`context_menu_picker_reachable_and_closes_on_selection`：

1. **布置**：`init_no_event_loop` + `AppWindow::new` + 1280×800 + `bridge::load_data`（同既有测试，
   menu_bridge::bind 不被 load_data 触发 → 测试自行注册 `menu.on_add_to_playlist` 捕获 (id) 列表、
   `player.on_play_tracks` 捕获 (start) 列表）；播种 30 行曲目（`source: 0`、`artist_mid/album_mid`
   置空避开行内链接层，同既有测试）；**另播种 `Data.sidebar_created` 两条歌单**
   （`PlaylistCover { id: "701"/"702", name: …, relation: "local", image: Image::default() }`，
   `source==0` 的 QQ 曲对任意 relation 可入，`playlist-allows` 放行）。
2. **打开**：右键 (640, 350)（行内命中，同既有测试坐标）→ 弹层面板几何（S6 像素标定复用）：
   面板左上 = 点击点 + (0, +4px) = (640, 354)，宽 216。
3. **切 picker**：左键完整点击「加入播放列表…」行（主模式行 3）。**坐标推导（评审必改 3：补
   分隔线 5px——行 3 前有一条 1px 分隔线 + 两侧各 2px spacing）**：行 3 中心 =
   `354 + 8(padding) + 36×3(前三行) + 2×3(前三行后 spacing) + 1(分隔线) + 2(分隔线后 spacing)
   + 18(半行高) = 497`，x 取面板中心 748。与 picker 公式同风格显式写全。修复前：release 即
   框架关弹层（S6 click3-both 实证）；修复后：弹层存活，面板切 picker。
   （交叉验证：S6 实测 lib-light 分隔线 y=466/507，本公式推算面板顶 344 时的分隔线 466/507——
   逐像素吻合。）picker 行 0 中心 = `354 + 8 + 36(头行) + 2 + 1 + 2 + 18 = 421`（同一推导式）。
4. **选歌单**：左键完整点击 (748, 421) → 断言 `add_to_playlist` 恰一次且 id == "701"。
   **该断言是「picker 存活」的行为级证明**：弹层若在第 3 步被关，此点击落在其下表格行
   （y=421 → 行 3）触发 `play-tracks(3)` 而 add-to-playlist 永不触发——修复前必红。
5. **整体关闭**：左键完整点击 (748, 560)（旧面板区下方、表格行 6 上）→ 断言 `play-tracks`
   新增一次（点击穿透到表格 ⇔ 弹层已关；若弹层未关，picker 高度 129px 底缘 483 < 560，点击在
   弹层外 → press 关弹层、点击不达表格 → 断言红）且 `add_to_playlist` 计数仍为 1。
6. **注释**：坐标公式全部写明推导（面板 padding 8 / 行高 36 / 行距 2 / 分隔线 1px+前后各 2px），
   几何漂移时有迹可循；行高 36px 点中心容 ±18px 容差，不贴边。

断言手段说明（评审可选 1 修订：主案行为断言，理由改为**不扩 API 面 / 不动 context-menu.slint**，
S1 冲突不再是考量——S1 的 PLAN 不含 track-table/queue-drawer/context-menu 任一改动，track-table
仅零改动引用，§⑥ 冲突已降级）：截图像素分析对 CI 过重（S6 的 check_shots.py 是验收工具不是回归
工具）；属性断言需给 TrackMenu global 新增 `picker-open` 暴露（改 context-menu.slint，扩 API 面）。
行为断言零新增 API、语义精确（add-to-playlist 只能来自 picker 行命中；play-tracks 探针证明弹层
关闭）。**回落方案**（若实施遇几何漂移 flake）：TrackMenu 加 `in-out property <bool> picker-open`，
面板在 `picker-mode` 两个赋值点同步写 global——+6 行，经评审确认**无 S1 冲突代价，可放心启用**。

可选加固（实施时视时长取舍，不作验收门槛）：ESC 关闭不回归（dispatch `WindowEvent::KeyPressed`
Escape 后做第 5 步同款探针）；弹层外点击关闭（第 5 步已隐式覆盖）。

### 项 B 单测（backend.rs `#[cfg(test)]`，`#[tokio::test]`，tokio macros 已在依赖特性内）

1. `request_times_out_when_daemon_silent`（镜像 CLI :180-211）：临时端点
   （`std::env::temp_dir().join(format!("hmp-desktop-timeout-{}", pid))`——仓库既有模式，不新增
   tempfile dev-dep；Windows 经 pipe_name 确定性映射管道名）；`IpcListener::bind` + 服务端任务
   accept → 读走完整请求帧 → `std::future::pending()`；`request_with_timeout(endpoint, Status, 150ms)`
   → 断言 `matches!(err, Timeout(d) if d == 150ms)` 且 `start.elapsed() < 1s`。
2. `request_completes_within_budget`（回声服务器，守护重构不破帧协议）：服务端 accept → 读帧 →
   回 `encode_frame(&Response::Ok)`；`request_with_timeout(endpoint, Status, 5s)` → 断言
   `Ok(Response::Ok)`。
3. `offline_reason_only_for_actionable_errors` 扩一 arm：`Timeout(30s) → None`（钉「不进可操作
   离线原因」语义）。

### 既有测试回归面

hmp-desktop 全部（context_menu / drag / hover_slider / hover_stretch / like_key / live_daemon(默认
ignore) / svg_icon + lib 单测）；hmp-cli 回归（Timeout 先例所在 crate）；改 IPC 链纪律要求的
全仓逐 crate（见 T4）。红→绿记录：T2 在 T1 之前先跑一次取红证（超时单测为新增代码，无红期，
以实现即测论证）。

---

## ⑥ 风险与回滚

| 风险 | 概率 | 缓解 | 回滚 |
|---|---|---|---|
| A：close-on-click-outside 引入未预期的框架行为差异（如某后端 ESC/点外实现偏差） | 低 | 关闭逻辑在 i-slint-core 单点实现、与后端无关（§②已核源码）；集成测试覆盖点外/整体关闭 | revert T1 两行（slint 无状态，即改即生效） |
| A：与并行流 S1 的 track-table.slint 改动合并冲突 | **低（评审可选 1 降级）**：S1 的 PLAN 不含 track-table/queue-drawer/context-menu 任一改动（track-table 仅作零改动引用），无同文件并发编辑 | 改动锚定仅 :297-312 一块 + 注释；如有意外冲突由协调者合并时拼合（close-policy 行必留） | 不需回滚，合并解决 |
| A：测试坐标随页面布局漂移误红 | 中 | 坐标公式注释化；行高 36px 中心点击容差大；漂移属「测试基建需更新」而非误报（既有测试同类约定） | 更新常量 |
| B：30s 对个别慢请求误伤（超慢网封面） | 低 | 上游 reqwest 15s 内必失败或成功，30s 余量 2×；CLI 同值已在线 | 调 REQUEST_TIMEOUT 单常量 |
| B：订阅首连 Status 探测超时传 Err 改变离线时序（挂死 daemon 场景由静默冻结变离线+2s 重试） | 低 | 该场景现状即坏（UI 永久冻结）；新行为=既有 daemon 死亡同路径，离线 toast 一次性、重连有界；无 Quit churn（超时不触发重启路径） | revert verify_backend_generation 两 arm（保留 request 超时不影响其余） |
| B：挂死时 restart_backend 经 Quit 最坏 ~42s | 低 | 有界（现状无限）；与 CLI quit 同策略 | 不需 |
| 回滚整体 | — | 两项各自独立提交（A 一个 commit、B 一个 commit），任一可单独 revert | — |

---

## ⑦ 边界确认

- **本流独占/授权文件（仅触碰以下）**：`crates/hmp-desktop/ui/track-table.slint`、`ui/queue-drawer.slint`、
  `crates/hmp-desktop/PORTING.md`（**仅 :188 ④ 子句**，评审必改 1 纳入）、`src/backend.rs`、
  `tests/context_menu_picker.rs`（新）、`crates/hmp-cli/src/client.rs`（**仅 :28-31 注释归因更正**，
  协调者追加授权）、`PLAN.md`。`ui/context-menu.slint` **计划不动**
  （回落「picker-open 属性断言」方案时才会触碰，届时最小 +6 行）。
- **禁碰清单遵守**：now-playing.slint / player_bridge.rs / stores.slint / bridge.rs / daemon 侧
  全部 / docs/** / .github/** —— 全部不碰。项 B 的用户可见错误展示复用 player_bridge 既有泛化
  `Err(e)` arm（warn 日志 + 笼统 toast），**无需也未被允许**改 player_bridge。
- **移交协调者的一件事**：
  - `quality-badge.slint:41` PopupWindow 维持默认 close-on-click：功能无碍（每行显式 close），
    若求全仓一致性可由 S1 补同一行 close-policy（观察项，非缺陷）。
  （原「PORTING.md:188 更正」移交项已按评审必改 1 收归本流 T1c，删除移交。）
- **协议与指纹**：项 B 不改 IPC 帧协议、不改 daemon 侧；desktop backend.rs 变更不影响
  `BUILD_CODE`（指纹来自 hmp-daemon 构建），但按 AGENTS 纪律仍重建 hmp + hmp-desktop 双二进制、
  `hmp quit` 清旧 daemon 后真机实测。
- **独立审计对照**：缺陷 #1「确证」状态与 S6 verify/REPORT.md 证据链一致（源码级复核已完成，
  见 §②——若独立审计推翻结论，协调者通知后裁掉项 A，项 B 不受影响）。
