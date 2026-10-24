# S11 计划评审笔记（二发独立审核，2026-10-24）

> 审核对象: PLAN.md @ worktree s11-deps-latest（308 行，基 a5a0236）。
> 方法: 只读评审 + 活体命令取证（worktree 保持脏现场未动）。cargo = ~/.cargo/bin/cargo。
> 本文件按维度增量 append，防驱逐丢工作。

## D1 活体验证（16 项跨 major 脏现场）

- `cargo check -j 4 --workspace --all-targets --all-features` → **exit 0**（Finished，1.88s 热缓存）。
  仅存噪音 = slint-build 的 `viewport-y` 弃用警告（build-script 输出，非 rustc lint），与 §5 头注一致。
- `cargo clippy -j 4 --workspace --all-targets --all-features -- -D warnings` → **exit 0**
  （1m44s；警告均为同一 viewport 弃用 1 duplicate 系列）。§5 双绿声称**复核通过**。
- git 脏面（14 文件）与 §5/§6.2 适配清单**逐一吻合**:
  - manifest 6 个: 根（rust-version 1.92、base64 0.23、slint 1.18、zbus 6、lofty 0.25、rusqlite 0.40、
    toml 1、reqwest 0.13 features=[json,cookies,gzip,brotli,deflate,query,form,rustls,webpki-roots,stream,
    system-proxy]、sha1 0.11、notify 8）、player（rodio 0.23）、desktop（slint/slint-build 1.18、resvg 0.48、
    dev i-slint-backend-testing 1.18、**dev serial_test="4"=试验态，Q1=A 后实施时应回 "3"**）、
    daemon（tray-icon 0.26、windows-sys 0.61、ksni 0.3）、mpris（删 zvariant）、smtc（windows 0.62）。
  - 源码 7 文件: service.rs（~70 行）、metadata.rs（~32 行）、core.rs（~41 行）、local.rs（7 处 lofty，
    含 2 处 test 内）、mpris_smoke.rs（2 处去 expect）、probe_output.rs（2 处 Display）、windows.rs（1 行
    get→join）。**§6.2 写「源码 6 文件」实列 7 路径——计数笔误（cosmetic）**。
  - 零适配 crate 的声称与"无对应源码 diff"吻合（notify/rusqlite/toml/sha1/base64/resvg/tray-icon/
    windows-sys 均 0 行）。
- 差异点: 现场为 serial_test 4 试验态 → 本活体双绿证明的是 **16 项态**；Q1=A 终态（serial_test 3.5.0，
  MSRV 1.68，源码 0 适配）未被本机直接编译过，依赖 §6.3 提交2 后矩阵重跑兜底（矩阵已含）。

## D2 rust-version 1.92 依据 + Q1=A

- `cargo metadata` rust_version 聚合（现场 lock）: top = **serial_test/serial_test_derive 4.0.1 → 1.93.1**；
  其余顶点 1.92（count@1.92 = 24，含 10 个 hmp-* workspace crate 自身申报 1.92）。
- serial_test 3.x 最新 = 3.5.0（registry cache 证实），rust-version = **1.68** → Q1=A 弃升后
  全图外部顶点回到 1.92，与 §0「slint 1.18 链 1.92」定实**自洽**。Q1=A 不抬顶**成立**。
- 附带: 聚合天然覆盖 aws-lc-rs/aws-lc-sys（reqwest 0.13 rustls provider）——均 ≤1.92，未在 top-8 出现，
  msrv job 1.92 无隐藏超限者（申明 MSRV 层面）。

## D3 §5 抽验 3 项（源码级）

1. **zbus 6 Builder 不返 Result**: service.rs diff 实证 `.name(BUS_NAME.to_owned())` 与两处 `.serve_at(...)`
   的 `?` 已去（infallible 化）；`zbus::zvariant::{OwnedValue,Value}` → `zbus::{OwnedVariant,Variant}`；
   mpris_smoke.rs proxy Builder `.destination()/.path()` 去 2 个 `.expect`。与 §5 行 1 一致。
2. **rodio Sink→Player**: core.rs diff 实证 `OutputStream→MixerDeviceSink`、`OutputStreamBuilder→
   DeviceSinkBuilder`、`StreamError→DeviceSinkError`、`Sink→Player`（`new()/connect_new()` 同名）、
   静默 sink `sample_rate().get()/channels().get()`、cpal `name()`→Display（含 `#[cfg(unix)]
   collect_output_candidates` 内 2 处——即残余风险②的"已写未编译"部分）。probe_output.rs 2 处 `to_string()`
   /`{device}`。与 §5 行 2 一致。
3. **reqwest 0.13**: 锁 0.13.1（metadata 证实）；`system-proxy` feature → `hyper-util/client-proxy-system`
   （reqwest-0.13.1/Cargo.toml:185 源证）。**no_proxy 相容性（源码级判定）**: reqwest 0.13.1
   `ClientBuilder::no_proxy()`（async_impl/client.rs:1410）= 清空 proxies + `auto_sys_proxy=false`
   ——「Clear all Proxies... also disables the automatic usage of the system proxy」→ cdn_client()
   （hmp-media/src/stream/source.rs:367）直连语义与 0.12 完全一致，且**不依赖 system-proxy 是否启用**。
   reader.rs:899 测试经 `cdn_client()` 复用生产客户端 → 回环 mock 路径同享 no_proxy 保护。
   语义差（非缺陷、计划已列 smoke）: qqmusic-api client.rs:47/54 两客户端未 no_proxy → 0.12=仅环境变量代理，
   0.13+system-proxy=环境变量+OS 系统代理（超集，Clash registry 场景行为增强，属预期向）。

## D5 deny 抽验（lock 级 = target 无关，比 cargo tree 更强）

- ansi_term / atty: Cargo.lock grep = **0 处**（cargo tree -i 亦报 no match）→ 判删**证实**。
- ksni: lock 仅 0.3.7 一版；`cargo tree --target x86_64-unknown-linux-gnu` 亦仅 0.3.7（0.2.2 出 Linux 图）
  → §5 终态复核推翻 T1 中间态结论的记载**属实**（slint 1.18 链已不再拉 ksni 0.2.2）。
- rustybuzz: lock = 0 处 → 判删**证实**。
- quick-xml 0.41.0 / time 0.3.55 在 lock → 修复版判删的条件成立（0.41/0.3.55 与计划记载一致；
  RUSTSEC 修复阈值本身无法离线复核，采信计划+deny.toml 旧注释互证）。
- 留 3: paste 1.0.15 ← lofty 0.25.4（cargo tree -i 实证）; ttf-parser 0.25.1 + owned_ttf_parser 0.25.1 在 lock;
  proc-macro-error 1.0.4 ← gtk3-macros 0.18.2 ← libappindicator 0.9.0 ← tray-icon 0.26.1（lock 全链在）。
  → 删 6 留 3 **全部证实**。
- **连带补查（计划未做，本审补上）——licenses 面**: cargo-deny 同时查 licenses；新入图包
  （aws-lc-rs 1.18.2 `ISC AND (Apache-2.0 OR ISC)`、aws-lc-sys 0.46.0 多项式含 MIT-0 仅在 OR 组内、
  cmake/muda/notify-types/rtrb/sqlite-wasm-rs/alsa/coreaudio-rs/arboard/fontique/ash 等）经
  SPDX 递归求值对照 deny.toml allow 清单: **NOT-SATISFIED = 0，无许可新增项**。
  （MIT-0 仅出现在 `(... OR MIT-0)` 组内，被同组 Apache-2.0 满足；slash 旧式 = OR。）
- bans: multiple-versions="warn"（muda 0.19/0.21 双版只 warn 不挡）。提交序 1→2→3 期间 deny job:
  提交1 后 rustybuzz 随 resvg 0.47 回图但 ignore 未删（宽容方向，安全）; 提交2 后多余 ignore 存在
  （cargo-deny 默认 unused-ignores=warn，非 deny）→ **无中途红风险**。

（D4 提交切分 / D6 残余风险与矩阵 → 见后续 append）

## D4 提交切分隐藏风险（评审判断）

1. **提交1 时点 slint 1.18 + resvg 0.47 相容性 —— 判定: 相容，无阻塞**。
   - 证据链: 终态 lock 仅 resvg 0.48.1 一版（我方 req 0.48 与 i-slint-compiler 1.18 的 resvg 依赖统一）。
     提交1 态 = 我方 resvg req 仍 "0.47" + slint 1.18 链（其 resvg 解析 0.48）→ lock 出现 resvg 0.47+0.48
     双版并存。cargo 允许 0.x 双版共存、各自独立编译；hmp-desktop 源码按 0.47 API 写（covers.rs 未动，
     即 main 现状，main 已证可用）；slint 侧用自家 0.48。**T1 试验正是该组合（resvg manifest 未动 +
     slint 1.18.1 lock）且 check/clippy/全套 test 三绿（§2 T1 记载）** → 提交1 态有实证先例。
   - 提交1 的两个**未经任何编译验证的新增量**（T1 未含）:
     a. clippy.toml msrv 1.85→1.92: 抬 msrv 会激活 msrv 门控的 clippy lints → 可能冒新警告致
        `-D warnings` 红。概率低、修复小，但属"提交1 最可能红点"，§6.3 矩阵每笔重跑可兜住。
     b. ui viewport-*→content-* 改名 10 处: **已从 i-slint-compiler 1.18.1 源码证实**为双向别名
        （builtin_elements.rs:1514-1517 `viewport-y <=> content-y` 等 4 条），content-* 为正名、
        方向保持（in/in-out 不变），机械改名安全。usage 实数 = 10（primitives.slint + viewport.slint
        两文件），与计划记载吻合。
2. **§3.0 与 §6.2 的 serial_test 内部矛盾（必改）**: §3.0（自称"照抄"）仍写 dev `serial_test "3"→"4"`，
   与 §6.1 Q1=A 裁决（留 3）和 §6.2 终版（15 项、serial_test 弃升）冲突。实施代理若照 §3.0 抄: manifest
   "4" + workspace rust-version 1.92 → MSRV-aware resolver 需 serial_test 4.x 中存在 ≤1.92 版本，
   4.0.1 唯一 4.x 且需 1.93.1 → **解析期即失败（或锁到不可用版本）**。§6.2 虽自我声明"替代 §3.1 草案"，
   但未声明覆盖 §3.0。必改: §3.0 的 serial_test 行按 Q1=A 划掉/加注。
3. 提交2 现场源与终态的一处偏差: 现场 hmp-desktop manifest 为 serial_test "4"（试验态）——实施时
   按 §6.2（非照抄现场 diff）落 "3"。Q1=A 终态（serial_test 3.5.0, MSRV 1.68, 0 适配）从未整图编译过，
   由 §6.3 提交2 后矩阵全量重跑覆盖（矩阵已列，接受）。
4. 提交2 内部 slint 家族归属含糊（cosmetic）: §6.2 提交1 说"slint 1.18 系"，提交2 又列 hmp-desktop
   "dev i-slint-backend-testing 1.18"——i-slint-backend-testing 属"slint 系"却落在提交2。caret "1.17"
   本就可解析 1.18.1，任何归属都能编译；建议实施时把 desktop 的 slint/slint-build/
   i-slint-backend-testing 三行统一入提交1，使提交1 自洽。
5. 提交3 deny 时序安全: 提交1 后 rustybuzz 随 resvg 0.47 回图 + 旧 ignore 仍在（宽容）; 提交2 后多余
   ignore 触发 unused-ignores=warn（默认 warn 非 deny）→ 三笔之间 deny job 无中途红。**已证**。

## D6 残余风险 + 验证矩阵 + clippy.toml 连带项

- 残余风险①（ksni_tray.rs unix+tray）: CI clippy job = ubuntu + `--all-features` → tray feature 开、
  ksni_tray.rs 参与编译+lint，覆盖点**真实存在**（ci.yml 实读）。残余风险②（core.rs unix 分支）:
  `#[cfg(unix)]` 非 feature 门控 → 全部 ubuntu job（fmt 除外）编译。判定: 兜底设计成立。
- msrv job 1.92 转正的连带构建要求: reqwest 0.13 rustls 默认 provider=aws-lc → aws-lc-sys 0.46
  build-deps = **bindgen + cmake + cc + pkg-config**（metadata 实取）→ CI 需 cmake+libclang。
  本机（cmake 4.2.1 + libclang）实建成功（check exit 0 即含 aws-lc-sys build）；GH ubuntu/windows
  runner 镜像预装 cmake/LLVM，风险低但为**新增 CI 构建面**，计划未着墨（可选建议补一句）。
- clippy.toml msrv=1.92 连带项: **必要且方向正确**（现场 clippy.toml 仍 1.85，本发 clippy 亦在警告中
  证实不一致）。但见 D4-2a: 抬 msrv 激活新 lint 的连锁是提交1 唯一未预验证点。
- cargo doc 面（矩阵缺口，轻）: CI test job 含 `cargo doc --no-deps -D warnings`，§6.3 矩阵未列 doc。
  改名类型（Sink/OutputStream/zvariant）经 grep 无 intra-doc 链接引用（`[`Sink`]` 等零命中）→
  风险趋零；建议矩阵补一行 doc 或依赖 CI 兜底（现状即 CI 兜底）。
- check-windows job 无 --all-features → win_tray.rs（tray-icon 0.26）CI 不编译，但本机 T2
  --all-features 已编译证 0 适配成立；CI 面缺口为 main 既有形态，非本计划引入。

## D-TESTS 定向测试复跑（本审亲跑，进行中）

- storage: **FAILED —— 73 passed / 1 failed**（cargo "error: test failed"，失败用例名待隔离复跑；
  注: 后台管道 exit 码显示 0 为 grep 的退出码，非 cargo）。**计划 §5 末行"跑动中，结果补记"从未补记——
  该失败即未闭环项，疑为计划第五发未等到结果即收口。** 待复跑定位用例名 + 判 flaky/真回归。
- player/mpris/smtc/api: 跑动中，结果 append 于下。

## D-TESTS 结果闭环（复跑定位）

- **storage 1 失败用例 = `local::tests::lyrics_tag_item_reads_back`**（local.rs:341 断言，
  left=None / right=Some("[00:01.00]embedded\n")）。player 16 ok / mpris 21 ok / smtc 4 ok /
  qqmusic-api 全套 ok（135+20+10+4+25+17+20+20）。
- **根因（lofty 0.25.4 源码取证，非 flaky）**: lofty-0.25.4/src/tag/item.rs ID3v2 映射表
  `"USLT" => UnsyncLyrics`（:256），且 :254 注释明言 ID3v2 无法合理支持 `ItemKey::Lyrics`
  （LRC/USLT 语义不可互换）。即 Id3v2 下 `insert_text(ItemKey::Lyrics)` 存入的项
  `get_string(ItemKey::Lyrics)` 取不回，正名是 **`ItemKey::UnsyncLyrics`**。
- **影响面穿透生产代码**: `read_embedded_lyrics`（local.rs:180）同用 `get_string(ItemKey::Lyrics)`
  → **lofty 0.25 下 MP3(ID3v2/USLT) 内嵌歌词静默读不出**（FLAC/VorbisComments 的 LYRICS 键映射
  不变、仍可读）。T1（lofty 0.21）全套测试绿 → 失败归因于跨 major 升级本身，非环境。
- **定性**: §5 lofty 行"storage 7 处"适配清单**不完整**——缺第 8 处: 歌词键读取需兼容
  UnsyncLyrics（建议 `get_string(ItemKey::UnsyncLyrics).or_else(|| get_string(ItemKey::Lyrics))`
  顺序按格式语义取舍），同测例同步改写。§5 末行"跑动中，结果补记"的坑即此处，计划在
  「计划完成标记」全勾的表象下遗留了一个未闭环红测试 + 未发现的生产行为回归。

## 评审结论（终）

**通过但需小改**。三笔切分/rust-version 1.92/deny 删 6 留 3/残余风险兜底设计均经独立复核成立；
但提交2 前置条件（定向测试绿）在活体现场**不成立**（storage 1 红 = lofty 歌词回归），且 §3.0 与
Q1=A 裁决存在自相矛盾的"照抄"指令。必改见终报。

### 事实清单（亲跑命令与退出码）
1. cargo check -j4 --workspace --all-targets --all-features → **exit 0**（热 1.88s，仅 viewport 弃用警告）
2. cargo clippy 同参 -D warnings → **exit 0**（1m44s）
3. cargo test -p hmp-storage -j1 → **FAILED**（73/1，lyrics_tag_item_reads_back）
4. cargo test -p hmp-player / hmp-mpris / hmp-smtc / hmp-qqmusic-api -j1 → **全 ok**
5. cargo metadata rust_version 聚合 → max=1.93.1(serial_test 4.0.1)；除此外顶点 1.92
6. cargo tree -i ansi_term / atty → 不在图；Cargo.lock grep ansi_term/atty/rustybuzz=0 处、ksni 仅 0.3.7
   （含 --target x86_64-unknown-linux-gnu 复核）
7. cargo tree -i paste → lofty 0.25.4 链在图；lock: ttf-parser/owned_ttf_parser 0.25.1、
   proc-macro-error 1.0.4←gtk3-macros 0.18.2←libappindicator 0.9.0、quick-xml 0.41.0、time 0.3.55
8. 全图 SPDX 许可对照 deny.toml allow → NOT-SATISFIED=0
9. git diff 14 文件与 §5/§6.2 清单吻合（serial_test 现场="4"=试验态，与 Q1=A 终态不符，属预期）
10. i-slint-compiler 1.18.1 builtin_elements.rs:1514-1517 证实 viewport-*<=>content-* 双向别名；
    ui/ 实数 10 处（primitives+viewport 两文件）
