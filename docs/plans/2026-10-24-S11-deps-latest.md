# S11 计划：依赖升级最新 + rust-version 定实 + msrv job 转正

> 状态: 盘点完成 / 试验进行中（本文件为**接力落盘**，被驱逐后继代理从此续接）
> 分支: chore/deps-latest（基 main a5a0236）
> 用户裁决: 「依赖直接升级最新」——rust-version 如实申报（不迁就 1.85 旧值）
> 环境: 本机 rustc 1.99.0 / cargo 1.99.0，cargo = ~/.cargo/bin/cargo；worktree 无 target/（冷构建）

## 0. 已完结结论（后发代理直接采信，勿重跑）

1. **rust-version 定实 = 1.92**（不是预估的 1.88）。
   - 证据 A: `cargo update --dry-run`（rust-version 临时置 1.92 后）全图 260 包**零** `requires Rust` 注解 → 1.92 即全图 MSRV 最大值。
   - 证据 B: slint 1.17.1 → 1.18.1（caret "1.17" 已涵盖，无需改 manifest req）注解 `requires Rust 1.92`，是链条顶点。
   - 附带: keyring 4.2.0 需 1.88、zbus 5.19 需 1.87、zvariant 5.15.1 需 1.87、femtovg 0.27 需 1.88、const-field-offset 0.2.1 需 1.88 —— 均被 1.92 覆盖。
   - cargo 1.99 MSRV-aware resolver 行为: rust-version=1.85 时锁"highest Rust 1.85 compatible"，但 slint 链无 1.85 兼容版本 → fallback 全取最新并注解。即当前 Cargo.lock 本就与 1.85 声明矛盾（ci.yml msrv job 注释所述）。
2. **slint 家族无需改 manifest**：workspace `slint = "1.17"`、hmp-desktop `slint = "1.17"`(+unstable-winit-030)、`slint-build = "1.17"`、dev `i-slint-backend-testing = "1.17"` 均为 caret req，`cargo update` 直接锁 1.18.1。**但 `unstable-winit-030` feature 在 1.18 是否仍存在/更名 = T1 编译验证点**。
3. **wiremock 0.6.5 已是最新**（crates.io max），无升级动作；「三雷」回归 = 既有用例（见 §4 验证矩阵）。

## 1. 依赖盘点表（locked @ a5a0236 → crates.io 最新，2026-10-24 查）

### 1a. semver 内自动升（`cargo update` 覆盖，manifest 不动）

| 依赖 | 锁定 | 最新 | 备注 |
|---|---|---|---|
| slint 全家 | 1.17.1 | 1.18.1 | **需 Rust 1.92**；图 224→260 包（新增 ash/vulkan、arboard、femtovg 0.27、fontique 0.11、移除 rav1e/exr 链） |
| tokio / tokio-util | 1.53.1 / 0.7.19 | 1.54.1 / 0.7.20 | 平滑 |
| serde_json | 1.0.151 | 1.0.154 | |
| clap | 4.6.6 | 4.6.7 | |
| image | 0.25.9 | 0.25.10 | |
| keyring | 4.1.6 | 4.2.0 | 需 1.88 |
| zbus | 5.18.0 | 5.19.0 | **6.1.0 已出**（跨 major 见 1b） |
| zvariant | 5.13.1 | 5.15.1 | 已弃用（并入 zbus 6） |
| thiserror / flate2 / futures-util / libc | 2.0.19 / 1.1.9 / 0.3.33 / 0.2.189 | 2.0.21 / 1.1.10 / 0.3.34 / 0.2.191 | |
| serde / tracing / tracing-subscriber / url / percent-encoding / bytes / tempfile / tracing 家族 | 已最新 | — | dry-run 不动 |

### 1b. 跨 major / 0.x 跳线（需改 manifest，T2 逐项试编译）

| 依赖 | 现版 | 最新 | 消费 crate | 预估破坏面 |
|---|---|---|---|---|
| rodio | 0.21.1 | 0.23.0 | hmp-player | Sink/Source API 中变 |
| rusqlite | 0.32 | 0.40.2 | hmp-storage, hmp-cli | 8 个 0.x major；基础 query API 稳 |
| notify | 6.1.1 | **8.2.0**（crates.io API 核实最新稳定；9.0.0-rc.6 预发布不采） | hmp-daemon | watcher/Event API 变 |
| reqwest | 0.12.28 | 0.13.5 | hmp-media, hmp-qqmusic-api, hmp-desktop | feature 名、代理/TLS 默认；涉 no_proxy 坑位 |
| zbus | 5.19 | 6.1.0 | hmp-mpris | zvariant 并入；Connection API 变 |
| zvariant | 5（直接依赖） | 5.15.1（终版） | hmp-mpris | zbus 6 下改用 re-export 或保留 5.15 |
| lofty | 0.21.1 | 0.25.4 | hmp-storage | tag 读写 API 变 |
| tray-icon | 0.24.2 | 0.26.1 | hmp-daemon (win) | TrayIconBuilder API 变 |
| ksni | 0.2.2 | 0.3.7 | hmp-daemon (unix) | Tray trait 变；**lock 已含 0.3.6 传递实例**（谁引入待 T1 查 `cargo tree -i`） |
| windows | 0.61 | 0.62.2 | hmp-smtc | feature 名通常稳 |
| windows-sys | 0.60 | 0.61.2 | hmp-daemon | 小 |
| resvg | 0.47 | 0.48.1 | hmp-desktop (covers.rs) | usvg API 变；slint 1.18 链自带 0.48 → 升级可消双版本 |
| sha1 | 0.10.7 | 0.11.0 | hmp-qqmusic-api, hmp-media | digest 0.11 生态联动 |
| base64 | 0.22.1 | 0.23.1 | workspace (api/media) | Engine API 微变 |
| serial_test | 3.5.0 | 4.0.1 | hmp-desktop (dev) | 小 |
| toml | 0.8（workspace req） | 1.1.8 | hmp-storage | lock 已含 toml 1.1.4（传递）；req 升 "1" 统一 |

### 1c. 不动项

- wiremock 0.6.5（已最新）；libc 保持 0.2（1.0.0-alpha 为预发布不采）；notify 不采 9.0.0-rc。
- path 依赖（hmp-core 等）不涉。

## 2. 试验记录（trial，跑完还原 git checkout -- .）

### T1 semver 全升（rust-version=1.92 + cargo update）— 已跑 update，clippy 进行中
- [x] cargo update 实跑: Cargo.lock 1289+/1385-；图 224→260 依赖闭包（lock 总 name 数 849）
- [x] cargo metadata rust_version 聚合: **max = 1.92**（24 个包声明 1.92；次高 1.90/1.89）→ §0 结论复核通过
- [x] ksni 0.3 来源: **i-slint-core 同时依赖 ksni 0.2.2 与 ksni 0.3.7**（slint 自身 Linux 托盘栈）
  → 即使我方 hmp-daemon 升 ksni 0.3，ksni 0.2.2 仍在树（经 slint）→ **deny 的
  ansi_term/atty（RUSTSEC-2021-0139/2024-0375）ignore 必须保留**（clap2→dbus-codegen 链随 ksni 0.2.2 存续）
- [x] toml 1.1.8 来源: **skia-bindings**（slint skia 渲染器 build 链）→ 我方升 toml "1" 可与之统一消双版本
- [x] deny 影响面（新 lock 实测）:
  - time 0.3.55 ≥0.3.47 → **RUSTSEC-2026-0009 ignore 可删**
  - zbus_xml 5.2.1 + quick-xml 0.41 → **RUSTSEC-2026-0194/0195 ignore 可删**
  - exr/rav1e/pulp 已随升消失，但 paste 1.0.15 经 **lofty 0.21** 存续 → RUSTSEC-2024-0436 暂留，
    **lofty 升 0.25 后复测**（若 lofty 0.25 弃 paste 则删）
  - rustybuzz 0.20.1/ttf-parser 0.25.1 经 **usvg（我方 resvg 0.47）** 存续 → **resvg 升 0.48 后复测**
  - ansi_term 0.12.1/atty 0.2.14 存续（ksni 0.2.2，见上）→ ignore 保留
- [x] clippy --workspace --all-targets --all-features -D warnings: **通过（exit 0，3m26s）**
  - `unstable-winit-030` feature 在 slint 1.18 仍存在（feature 解析未报错 = 验证通过）
  - 唯一噪音: **slint 1.18 编译器弃用警告**（build-script 输出，非 rustc lint，`-D warnings`
    不拦截、CI 不红）: `viewport-*` 属性 → `content-*`。
    **适配项（机械改名，10 处）**: `ui/primitives.slint` 9 处 + `ui/viewport.slint` 1 处
    （rust 侧无 `viewport` 引用）；弃用别名仍可用故非阻塞，但按仓库洁净纪律随提交 1 改掉。
- [x] 定向测试 = 全套 `cargo test --workspace -j 1`: **全绿（exit 0）**，所有 test result 均 ok、
  0 failed（含 desktop 弹层哨兵/虚拟化、media no_proxy、api/daemon wiremock 全部套件）。
  **结论: T1（semver 全升 + slint 1.18.1 + rust-version 1.92）零源码适配即全绿**（唯 viewport-* 弃用警告改名项）。

### T2 跨 major 合并试编译 — 锁定成功，check 进行中

**已确认（锁定层）**:
- reqwest 0.13 feature 改名: `rustls-tls` → **`rustls` + `webpki-roots`**；系统代理检测移入
  **`system-proxy`** feature（no_proxy 坑位相关，需显式启用以保 0.12 行为）。
  已按 `["json","cookies","gzip","brotli","deflate","rustls","webpki-roots","stream","system-proxy"]` 试锁成功。
- **reqwest 锁到 0.13.1**（0.13.5 可用但被 MSRV-aware resolver 压住，其 MSRV > 1.92，具体值未查）。
- **serial_test 4.0.1 要求 Rust 1.93.1** —— 超过 slint 顶点 1.92！**裁决点**（见 §6）。
- **aws-lc-rs/aws-lc-sys/cmake 新增构建依赖**（reqwest 0.13 rustls 默认 provider = aws-lc）；
  本机 cmake 4.2.1 在位可编译；CI ubuntu/windows runner 均预装 cmake。替代路径
  `rustls-no-provider`+ring 需自建 TLS config，代码量大，不推荐。
- zbus 6.1.0 吸收 zvariant（hmp-mpris 的 `zvariant = "5"` 已删，`zbus::zvariant` re-export 覆盖）。
- tray-icon 0.26.1: 新增 muda 0.21（菜单），Linux 仍 libappindicator（gtk 链未拆）；
  x11/libxdo 旧后端移除。
- notify 8.2.0: 拆出 notify-types 2.1；inotify 0.9→0.11、dirs 6→7 连带。
- rusqlite 0.40.2: libsqlite3-sys 0.30→0.38，新增 sqlite-wasm-rs/rsqlite-vfs/rtrb。
- rodio 0.23.0: cpal 0.16→0.18、alsa 0.11、coreaudio 0.14 连带。

**deny.toml ignore 终局判定（T2 锁定后实测）**:

| ignore | 判定 | 依据 |
|---|---|---|
| RUSTSEC-2021-0139 ansi_term | **删** | ansi_term/clap2/dbus-codegen 随 ksni 0.2.2 移除出图 |
| RUSTSEC-2024-0375 atty | **删** | 同上 |
| RUSTSEC-2026-0009 time | **删** | time 0.3.55 ≥ 修复版 0.3.47 |
| RUSTSEC-2026-0194/0195 quick-xml | **删** | zbus_xml 5.2.1 + quick-xml 0.41 |
| RUSTSEC-2026-0206 rustybuzz | **删** | resvg 0.48 弃 rustybuzz（usvg 0.47 同步移除） |
| RUSTSEC-2024-0436 paste | **留** | paste 1.0.15 经 **lofty 0.25.4** 仍在图 |
| RUSTSEC-2026-0192 ttf-parser | **留** | ttf-parser 0.25.1 经 owned_ttf_parser（slint femtovg 文本栈） |
| RUSTSEC-2024-0370 proc-macro-error | **留** | tray-icon 0.26.1 Linux 仍依赖 libappindicator→gtk3-macros |

- [x] cargo check --workspace --all-targets --all-features -j 4: **exit 0**（第五发收口，
  4 轮迭代修复: mpris_smoke/probe_output 示例 + reqwest query/form feature + lofty local.rs
  7 处——明细见 §5）; clippy -D warnings 亦 **exit 0**; 定向测试见 §5 末行

## 3. 实施步骤（实施代理照此执行）

### 3.0 manifest req 改动清单（T2 试编译验证后照抄）

workspace 根 `Cargo.toml`:
- `rust-version = "1.85"` → `"1.92"`（附一行注释: slint 1.18 链 MSRV 顶点，cargo metadata 聚合核实）
- `base64 "0.22"→"0.23"`、`rusqlite "0.32"→"0.40"`、`notify "6"→"8"`、`reqwest "0.12"→"0.13"`
- `toml "0.8"→"1"`、`sha1 "0.10"→"0.11"`、`slint "1.17"→"1.18"`（显式钉住新 minor，风格从旧值延续）

crate 级:
- hmp-player: `rodio "0.21.1"→"0.23"`
- hmp-desktop: `slint "1.17"→"1.18"`（feature unstable-winit-030 待 T1/T2 核实仍在）、
  `slint-build "1.17"→"1.18"`、dev `i-slint-backend-testing "1.17"→"1.18"`、dev `serial_test "3"→"4"`、
  `resvg "0.47"→"0.48"`
- hmp-daemon: `tray-icon "0.24"→"0.26"`、`windows-sys "0.60"→"0.61"`、`ksni "0.2"→"0.3"`
- hmp-mpris: `zbus`（workspace）随根升 "6"；`zvariant "5"` → 删除改用 zbus re-export（T2 核实路径名）
- hmp-smtc: `windows "0.61"→"0.62"`
- 不动: wiremock/serde/tracing*/tokio*/url/bytes/flate2/futures-util/tempfile/libc/percent-encoding/
  thiserror/image/clap/keyring（semver 内 cargo update 已覆盖）

### 3.1 提交切分（三笔，代码与 CI 分离、revert 粒度合理）

1. `chore(deps): semver 全量刷新 + rust-version 定实 1.92`
   - 触及: 根 Cargo.toml（rust-version）、Cargo.lock、（如有）slint 1.18 编译适配源码
   - 前提: T1 clippy/定向测试绿
2. `chore(deps): 跨 major 升级至最新（rodio/rusqlite/notify/reqwest/zbus/lofty/tray-icon/ksni/windows×2/resvg/sha1/base64/serial_test/toml）`
   - 触及: 上述 manifest req + 适配源码（最小化）+ Cargo.lock
   - 前提: T2 组合试编译通过 + 适配点全绿
3. `ci(msrv): msrv job 转正 1.92 + deny ignore 清理`
   - 触及: .github/workflows/ci.yml、deny.toml
   - ci.yml: `name: msrv (1.85)→msrv (1.92)`、`dtolnay/rust-toolchain@1.85→@1.92`、
     **删除 `continue-on-error: true`**、替换探针注释为定实说明（slint 1.18 链顶点 1.92，
     S11 裁决「直接升最新+如实申报」）
   - deny.toml: 删 `RUSTSEC-2026-0009`（time 0.3.55 已修复）、`RUSTSEC-2026-0194`/`RUSTSEC-2026-0195`
     （zbus_xml 5.2.1+quick-xml 0.41 已修复）；**保留** ansi_term/atty（ksni 0.2.2 经 slint 存续）；
     paste 与 rustybuzz/ttf-parser 按 T2 复测结果定夺；注释块同步改写
   - 本机无 cargo-deny → deny 验证 = CI 观察（deny job 已 blocking）

### 3.2 验证矩阵（每笔提交后跑；全绿才算落地）

| 项 | 命令 | 备注 |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | |
| clippy | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | |
| test | **逐 crate** `cargo test -p <crate> -j 1`（10 crate 全跑） | AGENTS 纪律：勿单包宣称全绿 |
| 坑位定向 | desktop: context_menu/context_menu_picker/drag/hover_slider/hover_stretch；media: stream reader/source（no_proxy）；api+daemon: wiremock 套件 | §4 清单 |
| release 双二进制 | `cargo build --release`（hmp + hmp-desktop 同源同指纹） | |
| 隔离 smoke | `hmp quit` 杀旧 daemon → 新 release hmp 播放/搜索/收藏走查 + 托盘 + SMTC 注册 | AUDIT §18/§22 陈旧 daemon 纪律 |
| MSRV | 本机装 1.92 工具链 `cargo +1.92 check --workspace --all-targets`（可选）或托付 CI msrv job | |
| deny | CI deny job 观察 | 本机无 cargo-deny |

### 3.3 风险与回滚

- **slint 1.18 行为回归**（弹层 close-policy / 虚拟化 / winit 0.31 事件）: 回退
  `cargo update -p slint --precise 1.17.1`（连带 i-slint-* 同步降级；rust-version 相应回 1.88——
  1.17.1 链 MSRV 顶点=1.88 const-field-offset 0.2.1，两值联动变更，需同 commit）。
- **reqwest 0.13 代理/TLS 语义变**: no_proxy 单测 + 手动系统代理（Clash）环境 smoke；回退 precise 0.12.x。
- **rodio 0.23 设备/缓冲变化**: release smoke 听感走查；回退 0.21.1。
- **zbus 6 MPRIS**: Linux CI（check-windows 不覆盖 mpris? mpris 非 target-gated、Windows 亦编译 zbus，
  clippy 覆盖编译面；运行面 playerctl 走查托付 Linux 环境/后续流）。
- **rusqlite 0.40 bundled SQLite 升版**: schema 无 migration 变更预期；storage/daemon 测试 + 首启
  建库 smoke 兜底。
- 通用: 三笔提交切分使任一回归可独立 `git revert`；锁文件级单包回退用 `--precise`。

## 4. 已知坑位复核清单（升级后必测）

1. **Slint 弹层 close-policy / 不重摆**（S7）: `crates/hmp-desktop/tests/context_menu*.rs`，哨兵
   `context_menu_picker_reachable_and_closes_on_selection`（context_menu_picker.rs:92）；S7 依据
   i-slint-compiler 1.17.1 lower_popups.rs close-on-click 语义 —— **slint 1.18 编译器行为可能变**。
2. **窗口化虚拟化 drag/hover**: `tests/drag.rs` / `hover_slider.rs` / `hover_stretch.rs`。
3. **wiremock 三雷**（S1 裁决 O3 + S4）: 匹配器按 api 层请求构造字段断言；tokio::test 常规形态
   （无 spawn_blocking 阻塞读、无 XDG 写）；涉及文件: api/tests/regression_browse.rs、daemon/tests。
4. **no_proxy**: 生产代码 `hmp-media/src/stream/source.rs:367 .no_proxy()`；测试注释
   `reader.rs:899`（系统代理劫持回环 mock）。reqwest 0.13 下 .no_proxy() 语义复核。

## 5. 跨 major 破坏面明细（T2 已试编译收口，第五发 2026-10-24）

> 终态: 16 项跨 major manifest 全改 + 源码适配 5 文件后
> `cargo check --workspace --all-targets --all-features -j 4` **exit 0**（1m24s）、
> `cargo clippy --workspace --all-targets --all-features -- -D warnings` **exit 0**（1m35s，
> 唯一噪音仍为 slint-build 的 viewport-* 弃用输出，非 rustc lint 不拦 -D）。
> 修复迭代史（本发 4 轮 check）: ① 前发已适配 mpris/player/smtc 源码但漏示例与 lofty；
> ② 补 mpris_smoke（proxy Builder infallible）+ probe_output（cpal Display）+ reqwest `query` feature；
> ③ 再补 reqwest `form` feature；④ 再补 lofty local.rs 7 处 → 全绿。

| 依赖 | 锁定版 | 试编译 | 适配量（我方源码） | 升/弃 |
|---|---|---|---|---|
| zbus 5→6 | 6.1.0 | 通过 | **mpris ≈104 行**: metadata.rs 全套改名 `zvariant::{Value,OwnedValue,ObjectPath,Array}`→`zbus::{Variant,OwnedVariant,ObjectPath,Array}`; service.rs connection `Builder::name()/.serve_at()` 不再返 Result（去 `?`）; example mpris_smoke.rs proxy Builder `.destination()/.path()` 返 `Self`（去 2 个 `.expect`） | 升 |
| rodio 0.21→0.23（cpal 0.16→0.18） | 0.23.0/0.18.2 | 通过 | **player ≈45 行**: core.rs `OutputStream→MixerDeviceSink`、`OutputStreamBuilder→DeviceSinkBuilder`、`StreamError→DeviceSinkError`、`Sink→Player`（含 `new()/connect_new()` 同名迁移）、静默 sink 采样率/声道 `as usize`→`.get() as usize`、cpal `DeviceTrait::name()` 移除→`Display`; example probe_output.rs 2 处 `name()`→`to_string()` | 升 |
| lofty 0.21→0.25 | 0.25.4 | 通过 | **storage 7 处**（仅 local.rs）: `get_string(s)(&ItemKey::X)`→按值 ×6; `Tag::year()` 移除→`get_string(ItemKey::Year).and_then(parse::<i64>)` ×1 | 升 |
| reqwest 0.12→0.13 | **0.13.1**（0.13.5 被 MSRV-aware resolver 压制，其 MSRV>1.92） | 通过 | **api/media/desktop 0 行源码**: `.query()/.form()` 移入 feature → workspace features 增 `"query","form"`; `rustls-tls`→`rustls`+`webpki-roots`+`system-proxy`（前发已改，no_proxy 语义保持需 system-proxy）; `.no_proxy()` 调用面未变 | 升 |
| rusqlite 0.32→0.40 | 0.40.2 | 通过 | storage/cli **0 行**（db.rs 基础 query API 未动） | 升 |
| notify 6→8 | 8.2.0 | 通过 | daemon **0 行**（watcher.rs 用法兼容） | 升 |
| tray-icon 0.24→0.26 | 0.26.1 | 通过 | daemon **0 行**（win_tray.rs Windows 路径已实证; 连带 muda 0.21） | 升 |
| ksni 0.2→0.3 | 0.3.7 | **Windows 盲区** | ksni_tray.rs `#[cfg(all(unix,feature="tray"))]` 不参与本机编译——**残余风险①** | 升（Linux CI 兜底） |
| windows 0.61→0.62 | 0.62.2 | 通过 | smtc **1 行**: `IAsyncOperation::get()`→`join()` | 升 |
| windows-sys 0.60→0.61 | 0.61.2 | 通过 | daemon **0 行** | 升 |
| resvg 0.47→0.48 | 0.48.1 | 通过 | desktop covers.rs **0 行**（`resvg::usvg::Options`/`Tree::from_str`/`tiny_skia` 路径未变） | 升 |
| sha1 0.10→0.11 | 0.11.0 | 通过 | api/media **0 行** | 升 |
| base64 0.22→0.23 | 0.23.1 | 通过 | **0 行**（Engine API 未变） | 升 |
| toml 0.8→1 | 1.1.8 | 通过 | storage config.rs **0 行** | 升 |
| serial_test 3→4 | 4.0.1 | 编译通过（1.99 链） | desktop **0 行**; **但其 MSRV=1.93.1 > 1.92** → §6.1 Q1 | **弃升（建议）** |
| slint 1.17→1.18 | 1.18.1 | T1 已全绿 | 0 源码（viewport-* 改名 10 处随提交1）; `unstable-winit-030` 仍在 | 升 |

**传递双版本归因（终态 lock 实测，均不可消）**: zbus 5.19.0 + zvariant 5.15.1 ← accesskit_unix ← i-slint-backend-winit 1.18（Linux 无障碍栈）; toml 0.8.2 ← system-deps/gtk-sys ← libappindicator ← tray-icon 0.26 Linux 链; base64 0.22.1 ← reqwest 0.13 自身; windows-sys 0.45~0.61 六版本并存为传递常态。

**deny 终态复核（终态 lock 实测，推翻 T1 中间态结论）**: ansi_term/atty/clap2/dbus-codegen/ksni 0.2.2 已**全部出图** → RUSTSEC-2021-0139、2024-0375 ignore **删**（T1 时 ksni 0.2.2 经 i-slint-core 存续的记录基于中间 lock，终态已换链消失）; paste 1.0.15（lofty 0.25.4 仍带）、ttf-parser 0.25.1、proc-macro-error 1.0.4 在图 → 对应 ignore **留**; time 0.3.55、quick-xml 0.41、rustybuzz（出图）→ ignore **删**。

**新发现适配项（前发与 §3.0 均漏）**: `clippy.toml` 的 `msrv = "1.85"` 与 rust-version 1.92 不一致（clippy 实按 1.85 检查，本发 clippy 运行已警告）→ **提交 1 连带改 clippy.toml msrv = "1.92"**。

**残余风险（Windows 本机 check/clippy 覆盖不到）**:
1. `crates/hmp-daemon/src/tray/ksni_tray.rs`（unix+tray 门控）——ksni 0.3 Tray trait 兼容性未验证，Linux CI job 兜底。
2. `crates/hmp-player/src/core.rs` `#[cfg(unix)] collect_output_candidates`（cpal 0.18 适配已写但未编译）——同上。
3. zbus 6 / rodio 0.23 / reqwest 0.13 运行面行为（playerctl、听感、系统代理）——编译绿≠行为绿，§6.3 smoke 项覆盖。

**定向测试**: storage/player/mpris/smtc/api 五 crate `cargo test -j 1` —— 跑动中，结果补记于此行下方。

## 6. 裁决点 + 提交切分（终版）+ 验证矩阵（终版）

### 6.1 需用户裁决（实施代理默认按建议项执行，除非用户另裁）

**Q1 serial_test 4 vs MSRV 1.92**: serial_test 4.0.1 声明 MSRV 1.93.1，超 rust-version=1.92。
- **A（建议，默认）**: 弃升，desktop dev 留 `serial_test = "3"`（锁最新 3.x）。理由: dev 工具不驱动
  全仓 MSRV; msrv job 1.92 可保持 `--all-targets`（含 dev-deps 编译）。
- B: 升 4.0.1 且 rust-version 提 1.93.1 —— 为一个 test helper 抬全仓申报 MSRV，违背「如实申报
  =实际依赖顶点」原则（顶点是 slint 链的 1.92），不取。
- C: 升 4.0.1 且 msrv job 降为不含 --all-targets —— 验证矩阵覆盖缩水，不取。

### 6.2 提交切分（终版三笔，替代 §3.1 草案；revert 粒度 = 三档独立）

1. `chore(deps): semver 全量刷新 + rust-version 定实 1.92 + slint 1.18`
   - 触及: 根 Cargo.toml（`rust-version "1.85"→"1.92"`、`slint "1.17"→"1.18"`）、
     **clippy.toml（msrv 1.85→1.92，§5 新发现项）**、Cargo.lock、
     ui/primitives.slint + ui/viewport.slint（viewport-*→content-* 机械改名 10 处）
   - 前提: T1 clippy/test 全绿（已证，§2 T1）
2. `chore(deps): 跨 major 升级至最新（15 项，serial_test 按 Q1=A 弃升）`
   - manifest 6 个: 根（base64 0.23 / rusqlite 0.40 / notify 8 / reqwest 0.13+features
     query,form,rustls,webpki-roots,system-proxy / toml 1 / sha1 0.11 / zbus 6）、
     hmp-player（rodio 0.23）、hmp-desktop（slint 1.18 系已在提交1、resvg 0.48、
     dev i-slint-backend-testing 1.18）、hmp-daemon（tray-icon 0.26 / windows-sys 0.61 /
     ksni 0.3）、hmp-mpris（**删 zvariant 行**）、hmp-smtc（windows 0.62）
   - 源码 6 文件（§5 明细）: hmp-storage/src/local.rs（lofty 7 处）、
     hmp-player/src/core.rs（rodio/cpal）、hmp-player/examples/probe_output.rs（2 处）、
     hmp-mpris/src/metadata.rs + service.rs（zbus 6）、hmp-mpris/examples/mpris_smoke.rs（2 处）、
     hmp-smtc/src/windows.rs（1 行）
   - Cargo.lock
   - 前提: T2 check + clippy 双绿（已证，§5 头注）+ §6.3 定向测试绿
3. `ci(msrv): msrv job 转正 1.92 + deny ignore 清理`
   - .github/workflows/ci.yml: `msrv (1.85)→msrv (1.92)`、`dtolnay/rust-toolchain@1.85→@1.92`、
     **删 `continue-on-error: true`**、探针注释替换为定实说明（slint 1.18 链顶点 1.92，
     cargo metadata 聚合核实 + S11 裁决「直接升最新+如实申报」）
   - deny.toml: **删 6 个** ignore——RUSTSEC-2021-0139（ansi_term 出图）、RUSTSEC-2024-0375
     （atty 出图）、RUSTSEC-2026-0009（time 0.3.55 已修复）、RUSTSEC-2026-0194/0195
     （quick-xml 0.41 已修复）、RUSTSEC-2026-0206（rustybuzz 出图）;
     **留 3 个**——RUSTSEC-2024-0436（paste 经 lofty 0.25.4）、RUSTSEC-2026-0192
     （ttf-parser 0.25.1 经 owned_ttf_parser/slint）、RUSTSEC-2024-0370（proc-macro-error 经
     tray-icon Linux gtk 链）; 顶部注释块同步改写（依据 §5 终态 lock 复核）
   - 本机无 cargo-deny → deny 验证 = CI deny job 观察（已 blocking）

### 6.3 验证矩阵（终版，在 §3.2 基础上收敛；每笔提交后跑，全绿才算落地）

| 项 | 命令 | 收敛点 |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | |
| clippy | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | T2 态已预跑绿（§5）; viewport 噪音属 build-script，改名将消 |
| test | 逐 crate `cargo test -p <crate> -j 1`（10 crate 全跑，勿单包宣称全绿） | T1 已全绿（semver 态）; 提交2 后全量重跑 |
| 定向（提交2 后立即） | storage/player/mpris/smtc/api 五 crate -j 1（适配面） + desktop 弹层哨兵/drag/hover（slint 1.18） + media no_proxy | §4 清单不变 |
| release 双二进制 | `cargo build --release`（hmp + hmp-desktop 同源同指纹） | |
| 隔离 smoke | `hmp quit` 杀旧 daemon → 新 release 走查: 播放/搜索/收藏/托盘/SMTC + **系统代理（Clash）环境 no_proxy** + Linux 侧 playerctl MPRIS | AUDIT §18/§22 |
| MSRV | `rustup install 1.92` 后 `cargo +1.92 check --workspace --all-targets` | Q1=A 时含 serial_test 3（MSRV 低）无碍 |
| deny | CI deny job 观察 | 本机无 cargo-deny |
| Linux 编译面 | CI ubuntu job（check/clippy/test）——**ksni_tray.rs 与 core.rs unix 分支唯一覆盖点**（§5 残余风险①②） | 提交2 后首推必看 |

### 6.4 回滚锚点（对 §3.3 的增量）

- 提交2 整体 revert 即回 semver 态（提交1 独立保留）。
- 单包微回退: `cargo update -p <pkg> --precise <旧版>`（reqwest 0.13.1→0.12.x 需同时回
  manifest 与 features 拼写; rodio 0.23→0.21.1 同理）。
- zbus 6 回 5: mpris 源码改名需反向重写（104 行），建议整体 revert 提交2 而非单包。

## 计划完成标记

- [x] §1 盘点 / §0 rust-version 定实（1.92）/ §2 T1 semver 全绿 / §4 坑位复核 —— 前四发已结
- [x] §5 跨 major 破坏面明细 —— 第五发: T2 check+clippy 双绿，16 项逐一定案（§5 表）
- [x] §6 裁决点（Q1 serial_test）+ 提交切分终版（3 笔）+ 验证矩阵终版 —— 第五发
- 试验态还原: §5/§6 结论落盘后 `git checkout -- .`（结论已在本文件，实验现场可弃）
- 待办移交实施代理: 按 §6.2 顺序三笔提交，Q1 默认 A，§6.3 矩阵逐项过，Linux CI 首看残余风险①②
