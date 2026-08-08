# QMC2 流式播放（TCP 回环解密代理）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让加密音质（`.mflac`/`.mgg`/`.mmp4`/`.mnac`）的播放链路做到与 QQ 音乐官方客户端一致的**流式体验**：边下边播（缓冲几秒即出声）、任意位置即时 Seek，而不是当前的"整文件下载 → 整文件解密 → 本地 file:// 播放"。

**Architecture:** 在 `hmp-media` 新增 `proxy` 模块：启动一个只监听 `127.0.0.1:0`（内核分配随机回环端口）的极简 HTTP/1.1 服务，按 GStreamer 发来的 `Range` 请求，从 CDN 按区间拉取对应密文、用 QMC2 流密码**按绝对 offset 就地解密该区间**后返回。QMC2 的 map/RC4 两种密码均按绝对偏移寻址（`plain[i] = cipher[i] ^ keystream(i)`，偏移一一对应），因此任意字节区间可独立解密——这是本方案可行的根本。`PlayerCore`（playbin）无需改动：播放 `http://127.0.0.1:port/` 与现在的 https 流走同一条 souphttpsrc 通道，Seek 自动变成 Range 请求。

**Tech Stack:** Rust 2024, tokio（TcpListener/oneshot/semaphore）, reqwest 0.12（Range 头 + 206 解析）, hmp-qqmusic-api::algorithms::qmc2（已有）, wiremock（CDN 模拟测试）。

## Global Constraints

- 只监听回环接口 `127.0.0.1`，端口用 0 让内核分配（不占固定端口、不对外暴露）。
- QMC2 可寻址性约束：明文偏移 X 对应密文文件偏移 X（`X < audio_len`），解密区间即读取密文文件同区间后按偏移就地解密；**绝不下发 footer 字节**（响应长度以 `audio_len` 为上限）。
- 明文音质（MP3/AAC）不经过代理，维持现状直接播 https URI。
- CDN 不支持 Range 或缺失 Content-Length 时，**回退**到现有整文件下载+解密路径（`decrypt::prepare_playable_at` / `prepare_playable_embedded_at`，返回 file://）——旧行为完整保留。
- 解密密钥来源与上一轮一致：优先接口 `ekey`；缺失时从尾部（QTag/STag）提取内嵌 ekey；两者皆无 → 该音质视为不可用（回退链继续）。
- 不新增第三方依赖：HTTP 服务手写极简解析（仅支持 GET/HEAD、单 Range、keep-alive、定长响应）；`bytes`、`url` 等已是 workspace 依赖，可直接用。
- `PlayerCore`（hmp-player-gst）、`hmp-mpris`、`hmp-core` **不得修改**；`hmp-qqmusic-api` 本计划不改（上一轮已完成）。
- 生命周期：`prepare_stream` 返回 `PreparedMedia { uri, _guard }`，guard 被 Drop 时关闭代理服务；CLI 在 `run()` 作用域持有，桌面在 `AppCore` 持有并在换曲/播放结束/退出时释放。
- `cargo fmt --all`、相关 crate `cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`（多次复跑稳定）必须通过。hmp-mpris 的预存 clippy 错误在基线即存在，与本计划无关，不修。
- 每个 Task 一个原子 commit；中文 doc 注释、ASCII 代码（仓库惯例）。

---

## File Structure

- Task 1（代理骨架）：`crates/hmp-media/src/proxy/mod.rs`、`crates/hmp-media/src/proxy/range.rs`、`crates/hmp-media/src/proxy/http.rs`
- Task 2（流式数据源）：`crates/hmp-media/src/proxy/source.rs`（探测/尾部/按区间拉取解密）+ `proxy/mod.rs` 的 `prepare_stream`/`PreparedMedia`/`MediaServer`；`crates/hmp-media/src/lib.rs` 导出
- Task 3（CLI）：`crates/hmp-cli/src/play.rs`
- Task 4（桌面）：`crates/hmp-desktop/src/app.rs`
- Task 5（文档）：`docs/PROJECT.md`、`docs/QQMUSIC_PORTING.md`

---

### Task 1: 代理骨架 —— Range 解析 + 极简 HTTP/1.1 服务

**Files:**
- Create: `crates/hmp-media/src/proxy/mod.rs`（`pub mod http; pub mod range;` + 预留 `source` 的 pub 声明，Task 2 填充）
- Create: `crates/hmp-media/src/proxy/range.rs`
- Create: `crates/hmp-media/src/proxy/http.rs`

**Interfaces（Task 2 依赖，签名必须一致）:**
- `range.rs`：
  - `pub struct ByteRange { pub start: u64, pub end: u64 }`（闭区间，含两端）
  - `pub fn parse_range(header: &str, total: u64) -> Result<ByteRange, RangeError>` —— 仅支持单区间 `bytes=a-b` / `bytes=a-` / `bytes=0-`；`end` 缺失时取 `total-1`；`start > end` 或 `start >= total` → `Err(RangeError::Unsatisfiable)`；后缀式 `bytes=-N`、多区间、非法格式 → `Err(RangeError::Malformed)`（服务端按 416 处理 Malformed 与 Unsatisfiable 皆可，见 http.rs 约定——统一 416 并附 `Content-Range: bytes */{total}`）
  - `pub fn clamp_end(start: u64, end: u64, audio_len: u64) -> ByteRange` —— 请求区间超出 `audio_len` 时截断到 `audio_len-1`（footer 防护）；`start >= audio_len` 时返回 `ByteRange { start, end: start }` 由调用方判 416
- `http.rs`：
  - `pub async fn serve(listener: tokio::net::TcpListener, source: std::sync::Arc<dyn Source>, stop: tokio::sync::oneshot::Receiver<()>)` —— accept 循环；每连接 spawn 一个任务循环处理请求直到 `Connection: close` 或对端断开
  - `pub trait Source: Send + Sync { fn audio_len(&self) -> u64; async fn read_range(&self, range: ByteRange) -> std::io::Result<std::borrow::Cow<'_, [u8]>>; }` —— 返回该区间的**明文**字节（Task 2 实现：CDN 拉取+解密；测试时用假实现）
  - 请求处理规则：
    - 仅接受 `GET`/`HEAD`；请求行 `GET <path> HTTP/1.1`，忽略 path（一律服务流）；其他方法 → `405`；解析失败 → `400`
    - 读请求头直到空行（`\r\n\r\n`）；`Content-Length` 请求体（POST 等）不支持 → 405 已覆盖
    - 无 `Range` 头：`200` + `Content-Length: audio_len` + 流式发送全部明文（`HEAD` 只发头）；有 `Range`：解析 → `clamp_end` → `start >= audio_len` → `416` + `Content-Range: bytes */{audio_len}`；否则 `206` + `Content-Range: bytes {start}-{end}/{audio_len}` + `Content-Length: {len}` + 明文区间体
    - 所有响应带 `Accept-Ranges: bytes`、`Content-Type: application/octet-stream`；HTTP/1.1 默认 keep-alive，`Connection: close` 时响应后关闭连接
    - 写响应用 `tokio::io::AsyncWriteExt`，写完 flush；连接读取 EOF → 结束
  - `pub fn status_line(code: u16) -> &'static str`（200/206/400/405/416 的 reason phrase，供测试断言）

**测试（range.rs 纯函数 + http.rs 无网络单测，用 tokio TcpListener 绑定临时回环端口 + tokio TcpStream 手写 HTTP 客户端发请求）:**
- `parse_range_forms`：`bytes=0-` → `{0, total-1}`；`bytes=100-199` → `{100,199}`；`bytes=100-` → `{100,total-1}`；`bytes=-50` → Malformed；`bytes=1-2,3-4` → Malformed；空头 → Malformed（或约定 None——定：`parse_range` 不接受空，调用方先判无 Range 再调用）
- `parse_range_unsatisfiable`：`bytes=200-300` 且 total=100 → Unsatisfiable；`start > end` → Unsatisfiable
- `clamp_end_caps_at_audio_len`：`{0, 1<<40}` + audio_len=100 → `{0, 99}`；`{50, 60}` + audio_len=100 → 不变；`{100, 120}` + audio_len=100 → `{100,100}`（调用方判 416）
- `serve_serves_full_body_without_range`：假 Source（固定 audio_len + 内容），GET 无 Range → 200、正文全量
- `serve_serves_range_206`：GET `Range: bytes=5-9` → 206、`Content-Range: bytes 5-9/100`、正文 == 明文字节 5..=9
- `serve_416_beyond_audio`：`Range: bytes=100-`（audio_len=100）→ 416 + `Content-Range: bytes */100`
- `serve_caps_range_at_audio_len`：`Range: bytes=95-` → 206 且正文只到 99
- `serve_head_no_body`：HEAD → 200、有 Content-Length、正文空
- `serve_keepalive_two_requests`：同一 TCP 连接发两个 GET（第二个带 Range）→ 两次都正确响应且连接未断
- `serve_rejects_unsupported_method`：POST → 405
- `serve_stops_on_shutdown`：drop oneshot sender → accept 循环退出（用一个已完成标记验证）

**步骤：** 先写测试（失败）→ 实现 → 跑通 → `cargo fmt`/`cargo clippy -p hmp-media --all-targets -- -D warnings`/`cargo test -p hmp-media` → commit：
`feat(media): add range-addressable streaming proxy skeleton (Range parsing + minimal HTTP/1.1 server)`

---

### Task 2: 流式数据源 —— CDN 探测/尾部/区间解密 + prepare_stream

**Files:**
- Create: `crates/hmp-media/src/proxy/source.rs`
- Modify: `crates/hmp-media/src/proxy/mod.rs`（导出 `source` 与 `prepare_stream`/`PreparedMedia`）
- Modify: `crates/hmp-media/src/lib.rs`（`pub mod proxy;` + `pub use proxy::prepare_stream; pub use proxy::PreparedMedia;`）
- Modify: `crates/hmp-media/src/decrypt.rs`（把 `embedded_ekey(path, audio_len)` 重构为 `pub(crate) fn embedded_ekey_from_bytes(bytes: &[u8], audio_len: usize) -> Result<String, MediaError>`，原文件版转调它——供 source.rs 从尾部字节提取内嵌 ekey）

**Interfaces:**
- `pub struct PreparedMedia { pub uri: String, _guard: MediaGuard }`（`MediaGuard` 持 `Option<tokio::sync::oneshot::Sender<()>>`，`Drop` 时发停止信号；`uri` 为 `http://127.0.0.1:<port>/stream` 或回退的 `file://`）
- `pub async fn prepare_stream(url: &str, ekey: Option<&str>, progress: Option<&tokio::sync::watch::Sender<Option<f64>>>) -> Result<PreparedMedia, MediaError>`
  - 内部流程：
    1. `ekey = ekey.filter(非空)`；请求 `HEAD` 拿 `Content-Length`（失败→回退）；再 `GET Range: bytes=0-0` 探测：期望 `206` 且 `Content-Range: bytes 0-0/{total}` 且 total>0；不满足（200/无 Content-Range/无长度）→ **回退**：走 `decrypt::prepare_playable_at(root, url, ekey, progress)`（ekey 有值）或 `decrypt::prepare_playable_embedded_at`（ekey 为空），返回 `PreparedMedia { uri: file://, _guard: MediaGuard(None) }`
    2. 拉尾部：`tail_len = min(total, 0x40)`；`GET Range: bytes={total-tail_len}-{total-1}` → 尾部字节；`detect_footer(total, &tail)`：
       - `QTag{audio_len}`：若 `audio_len + 8 < total` 且 `(total - 8 - audio_len) > 0x40` 说明 ekey 文本区超出 0x40 窗口，需再拉一次精确尾部 `bytes={audio_len}-{total-1}` 重新 detect（两次为上限）；audio_len 取最终值
       - `V1{audio_len}`：同上即可
       - `None`：`audio_len = total`（无 footer）
    3. 密钥：`ekey` 有值 → `qmc2::decrypt_factory(ekey)`；无值 → `embedded_ekey_from_bytes(&tail_bytes, audio_len)`（QTag 取 meta 区首个逗号前文本；V1 先 utf8+`parse_ekey` 再 `parse_ekey_decoded`）→ 失败则回退到 `prepare_playable_embedded_at`（保留旧行为，别丢）或直接 `Err(Unsupported)`——**定：回退到 prepare_playable_embedded_at**，与今日行为一致
    4. 构建 `StreamSource { client, cdn_url, cipher: Arc<dyn Qmc2Cipher + Send + Sync>, audio_len, total_len, sem: Arc<Semaphore(4)> }`
    5. `TcpListener::bind("127.0.0.1:0")` → 取本地地址端口 → `tokio::spawn(http::serve(listener, Arc::new(source), stop_rx))` → 返回 `PreparedMedia { uri: format!("http://127.0.0.1:{port}/stream"), _guard: MediaGuard(Some(stop_tx)) }`
- `impl Source for StreamSource`：
  - `audio_len()` → audio_len
  - `read_range(r)`：`sem.acquire`；`GET cdn_url` 带 `Range: bytes={start}-{end}`：
    - `206` → 读 body 全部字节 → 用 `cipher.decrypt(start, &mut buf)` 就地解密（分块，offset 累加）→ 返回
    - `200`（CDN 忽略 Range）→ 读全量 → 取 `[start..=end]` 切片 → 解密返回（防御性，正常探测已挡）
    - 其他状态 → `Err(io::Error)`（代理侧转 502，见 http.rs 约定：Source Err → 响应 `502`）
  - 注意 `end - start + 1` 可能很大（无 Range 全量请求），按 256 KiB 分块读+解密+写

**测试（wiremock 模拟 CDN + reqwest 当代理客户端）:**
- 构造辅助：`make_encrypted(plaintext, key, with_footer)` 已在 decrypt.rs 测试模块存在——**将其提升为 `#[cfg(test)] pub(crate)` 或复制**到 proxy 测试模块（定：抽到 `crates/hmp-media/src/testutil.rs`，decrypt 测试与 proxy 测试共用，`#[cfg(test)]`）；wiremock 支持 `header("Range", ...)` 匹配与 `ResponseTemplate::new(206).insert_header("Content-Range", ...)`
- `prepare_stream_serves_decrypted_range`：CDN mock（Range 支持）→ `prepare_stream` → `reqwest::get(uri)` 带 `Range: bytes=0-4095` → 明文前 4096 字节一致；`bytes=5000-6000` 一致（seek 行为）
- `prepare_stream_open_ended_range`：`bytes=0-` → 全量明文（可用较小 plaintext 如 8 KiB）
- `prepare_stream_caps_at_audio_len`：带 footer 的文件，`bytes=0-` 返回长度 == audio_len 且无 footer 字节
- `prepare_stream_416_out_of_range`
- `prepare_stream_seek_back_after_forward`：先 `bytes=5000-6000` 再 `bytes=0-1000` → 各自正确（无状态污染）
- `prepare_stream_falls_back_without_cdn_range`：CDN 对 Range 一律回 `200` 全量 → `prepare_stream` 返回 `file://`，内容为全量明文
- `prepare_stream_embedded_ekey_from_tail`：带 QTag 尾部、无 API ekey → 代理正常服务（内容一致）
- `prepare_stream_guard_drop_stops_server`：drop `PreparedMedia` → 端口不再接受连接（再次 connect 失败）
- `prepare_stream_keeps_alive`：一个 reqwest Client 连发两个 Range 请求成功

**步骤：** 先重构 `embedded_ekey_from_bytes` + 建 `testutil`（跑原测试保绿）→ 写失败测试 → 实现 source.rs/mod.rs/lib.rs → 全绿 → `cargo fmt`/clippy/`cargo test --workspace`（3 次）→ commit：
`feat(media): add CDN-backed range streaming proxy (probe, footer, on-demand decrypt)`

---

### Task 3: CLI 接线（hmp play 流式播放）

**Files:**
- Modify: `crates/hmp-cli/src/play.rs`

**步骤：**
- 加密音质分支的 `prepare_playable`/`prepare_playable_embedded` 调用替换为 `hmp_media::prepare_stream(&remote_uri, ekey, progress).await`，返回 `PreparedMedia`：
  - `let prepared = ...;` `let uri = prepared.uri.clone();` 在 `run()` 作用域持有一个 `let _media = prepared;`（guard 存活到 run 结束）
  - `prepare_stream` 返回 `file://` 时即旧回退路径，行为不变
- 输出文案：成功 → `println!("流式播放（QMC2 解密代理）: {uri}")`；progress 通道保留传入（回退路径仍显示进度；代理路径不产生进度，无妨）
- `LoadRequest.uri`/`Track.url` 用新 uri（`http://127.0.0.1:port/...` 或 `file://`），其余不动
- 校验：`cargo build -p hmp-cli`、`cargo clippy -p hmp-cli --all-targets -- -D warnings`（仅看 hmp-cli 自身告警，hmp-mpris 预存错误忽略）、`cargo test --workspace`
- commit：`feat(cli): stream encrypted formats through local QMC2 decrypt proxy`

---

### Task 4: 桌面接线

**Files:**
- Modify: `crates/hmp-desktop/src/app.rs`

**步骤：**
- `resolve_stream` 返回值改为携带 guard：定义
  ```rust
  struct ResolvedStream {
      file_type: SongFileType,
      uri: String,
      media: Option<hmp_media::PreparedMedia>, // 加密流持有 guard；明文为 None
  }
  ```
  `resolve_stream(...) -> Option<ResolvedStream>`；加密分支 `prepare_stream(...).await` 成功 → `media: Some(prepared), uri: prepared.uri`；失败 → continue（回退链）；明文 → `media: None`
- `ResolvedPlayback` 增加字段 `media: Option<hmp_media::PreparedMedia>`；两处 `resolve_play_request` 构造时从 `ResolvedStream` 带入
- `AppCore` 增加字段 `active_media: Option<hmp_media::PreparedMedia>`；`finish_play` 里 `self.active_media = resolved.media;`（旧 guard 自动 Drop → 旧代理关闭）
- 播放结束释放：AppCore 的事件循环已订阅 `state_rx`/`events_rx`（`PlayerEvent::PlaybackEnded`）——在相应处理处 `self.active_media = None;`（若现有循环未订阅 events，则改订阅 state 的 Ended 状态；实现时选最贴近现有结构的方式，并在报告中说明）
- 退出释放：`AppCore` 无显式 Drop 需求（字段 Drop 即释放），无需额外处理
- 校验：`cargo build -p hmp-desktop`、`cargo test -p hmp-desktop`、`cargo test --workspace`、fmt；clippy 同上限定
- commit：`feat(desktop): stream encrypted formats through local QMC2 decrypt proxy`

---

### Task 5: 文档

**Files:**
- Modify: `docs/PROJECT.md`
- Modify: `docs/QQMUSIC_PORTING.md`

**步骤：**
- `docs/PROJECT.md` §7.3 加密音质段：把"取流后解密为本地缓存文件播放"改为"经本地回环解密代理（127.0.0.1 随机端口，Range 按需解密）流式播放，支持边下边播与即时 Seek；CDN 不支持 Range 时回退整文件解密缓存"；§8.2 播放流程加一句"加密音质经本地解密代理（http://127.0.0.1:port）按 Range 取明文，Seek 即 Range 重定位"
- `docs/QQMUSIC_PORTING.md`：模块映射表 `hmp-media` 行备注追加"（含 proxy：回环 Range 解密代理）"；"加密取流"实测记录补一句代理流式链路
- commit：`docs: document streaming playback via local decrypt proxy`

---

## 自检（Self-Review）

- 覆盖：Task 1（Range 解析/HTTP 服务/生命周期）→ Task 2（CDN 探测/尾部/区间解密/回退/内嵌 ekey）→ Task 3/4（CLI/桌面接线，guard 生命周期正确）→ Task 5（文档）。
- 类型一致性：`parse_range(&str, u64) -> Result<ByteRange, RangeError>`、`clamp_end(u64, u64, u64) -> ByteRange`、`http::serve(TcpListener, Arc<dyn Source>, oneshot::Receiver<()>)`、`Source::{audio_len, read_range}`、`prepare_stream(&str, Option<&str>, Option<&watch::Sender<Option<f64>>>) -> Result<PreparedMedia, MediaError>`、`PreparedMedia { uri, _guard }` 在 Task 1→4 中签名一致。
- 已知限制（文档记录）：CDN URL 约 2 小时过期，长会话超时后 seek 会失败（重新播放即重新取流，属后续优化）；无解密区间内存缓存（来回 seek 重复解密，属后续优化）；代理为手写 HTTP 子集，仅服务 GStreamer 的 GET/HEAD+单 Range 行为。
- 验收：明文路径行为不变；加密路径启动即播 + 任意 Seek；CDN 无 Range 时回退旧路径；`cargo test --workspace` 稳定。
