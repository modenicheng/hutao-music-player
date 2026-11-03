//! 进程内随机访问解密 reader：同步 [`Read`] + [`Seek`]，供解码器
//! （rodio `Decoder` 约束 `Read + Seek`）直接消费；后台 tokio 生产任务
//! 驱动 [`super::source::fetch_and_decrypt_range`] 按需拉取并解密。
//!
//! # 架构
//!
//! - 共享状态：`Mutex<ReaderState>` + 消费侧 `Condvar`（数据到达唤醒）+
//!   生产侧 `tokio::sync::Notify`（seek / drop / 消费进度唤醒）。
//! - 消费者（解码线程）：找到覆盖 pos 的分块 → 拷贝返回并按回看上限修剪
//!   队首；pos == audio_len → `Ok(0)`；粘滞错误 → `Err`；否则 condvar 等待。
//! - 生产者（tokio 任务）：顺序预取（区间请求粒度 `params.chunk`），未读
//!   预取量达 `params.prefetch` 暂停；epoch 变化（窗口外 seek）时丢弃在飞
//!   请求（reqwest 响应流随 future drop 取消）在目标偏移重启。
//! - Drop（换曲/停止）：置 closed，生产者退出；tee 缓存收尾或后台补齐。
//!
//! # 不变量
//!
//! ① 生产者绝不跨 `.await` 持有 `std::sync::MutexGuard`：持锁段内只有
//!   内存操作与页缓存顺序小写；跨 await 持锁会把网络等待摊派给解码线程
//!   的每个 `read`。
//! ② 本 reader 的 `Read` 只会被解码器线程（spawn_blocking 探测 + 解码
//!   线程）阻塞调用，消费侧单线程假设成立；`Mutex` 仅用于与生产者同步，
//!   不承担多消费者语义。
//! ③ 错误语义对齐原回环代理：首个分块失败 = 装载错误（探测 read 直接
//!   `Err`），中途失败 = 解码线程 `Read` `Err`；错误置入共享状态后保持
//!   粘滞，直到窗口外 seek 复位（换偏移重试）。

use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use futures_util::StreamExt;
use futures_util::future::{Either, select};
use hmp_qqmusic_api::algorithms::qmc2::Qmc2Cipher;
use tokio::sync::{Notify, Semaphore};
use tracing::{debug, warn};

use super::source::{ByteRange, OwnedChunkStream, fetch_and_decrypt_range};
use crate::cache::{self, extension_from_magic, final_path, tmp_path};

/// 默认区间请求粒度：1 MiB。
const DEFAULT_CHUNK: u64 = 1024 * 1024;
/// 默认未读预取上限：4 MiB（达到后生产者暂停，等消费进度唤醒）。
const DEFAULT_PREFETCH: u64 = 4 * 1024 * 1024;
/// 默认队首回看保留：1 MiB（窗口内 seek 回看不触发 CDN）。
const DEFAULT_RETAIN: u64 = 1024 * 1024;

/// 预取/回看参数（测试可注入小值以确定性驱动窗口行为）。
#[derive(Clone, Copy)]
pub(crate) struct ReaderParams {
    /// 区间请求粒度（字节）。
    pub chunk: u64,
    /// 未读预取上限：达到后生产者暂停。
    pub prefetch: u64,
    /// 队首回看保留上限（字节）。
    pub retain: u64,
}

impl Default for ReaderParams {
    fn default() -> Self {
        Self {
            chunk: DEFAULT_CHUNK,
            prefetch: DEFAULT_PREFETCH,
            retain: DEFAULT_RETAIN,
        }
    }
}

/// tee 缓存回写计划（`prepare_media` 预计算；实际武装在 reader 打开时，
/// 逐打开做 INFLIGHT 去重与 final 命中检查）。
pub(crate) struct TeePlan {
    /// 缓存根（`cache_dir()/decrypted`）。
    pub root: PathBuf,
    /// 稳定缓存键（`cache_key(url, ekey.unwrap_or(""))`）。
    pub key: String,
}

/// 拉流上下文：reader、生产者任务与 tee 补齐任务共享。
pub(crate) struct SourceContext {
    pub client: reqwest::Client,
    pub cdn_url: String,
    pub cipher: Arc<dyn Qmc2Cipher>,
    /// 解密后的音频长度（剥离 footer 后）。
    pub audio_len: u64,
    /// CDN 上的原始文件总长。
    pub total_len: u64,
    /// 并发限制（最多 4 个并行 range 请求）。
    pub sem: Arc<Semaphore>,
    /// 边播边缓存计划；`None` = prepare 阶段已决定不缓存。
    pub tee: Option<TeePlan>,
    pub params: ReaderParams,
}

// ── 共享状态 ─────────────────────────────────────────────────────────

struct Shared {
    state: Mutex<ReaderState>,
    /// 消费侧：等待数据/错误/eof 到达。
    data_ready: Condvar,
    /// 生产侧：seek（epoch 变化）、drop（closed）、消费进度（暂停解除）。
    producer_ctl: Notify,
}

struct Chunk {
    /// 全局明文偏移。
    offset: u64,
    bytes: Vec<u8>,
}

impl Chunk {
    /// 结束偏移（不含）。
    fn end(&self) -> u64 {
        self.offset + self.bytes.len() as u64
    }
}

struct ReaderState {
    /// 已解密分块队列（带全局偏移，支持窗口内回看）。
    chunks: VecDeque<Chunk>,
    /// 已拉取到的明文偏移（不含）；窗口上界。
    fetched_until: u64,
    /// 当前生产起点（窗口外 seek 设定；队列空时它 == fetched_until）。
    fetch_from: u64,
    /// 消费位置（生产者暂停判断用）。
    consumed_until: u64,
    /// 代际：窗口外 seek 递增，生产者据此丢弃陈旧工作。
    epoch: u64,
    /// 生产者拉满 audio_len。
    eof: bool,
    /// 粘滞错误（窗口外 seek 复位）。
    error: Option<io::Error>,
    /// reader 已 drop：生产者退出。
    closed: bool,
    /// 生产者已退出（测试观测）。
    producer_done: bool,
    /// 边播边缓存状态（未武装/已收尾时为 None）。
    tee: Option<Tee>,
}

impl ReaderState {
    fn push_chunk(&mut self, offset: u64, bytes: Vec<u8>) {
        self.fetched_until = offset + bytes.len() as u64;
        self.chunks.push_back(Chunk { offset, bytes });
    }
}

// ── DecryptReader ────────────────────────────────────────────────────

/// 进程内随机访问解密 reader（生产入口见 [`super::source::StreamSource`]）。
pub(crate) struct DecryptReader {
    ctx: Arc<SourceContext>,
    shared: Arc<Shared>,
    /// 消费者私有位置（仅消费者线程读写，无锁）。
    pos: u64,
    /// 构造时（tokio 上下文中 `open`）捕获的 runtime：drop 可发生在解码
    /// 线程，仍能 spawn 收尾/补齐任务。
    runtime: tokio::runtime::Handle,
}

impl DecryptReader {
    /// 构造 reader 并在生产者任务上启动预取。
    ///
    /// 必须在 tokio 上下文中调用（[`hmp_core::MediaStreamSource::open`]
    /// 的契约：调用点在播放器的异步装载任务里）。
    pub(crate) fn spawn(ctx: Arc<SourceContext>) -> io::Result<Self> {
        let runtime = tokio::runtime::Handle::current();
        let tee = ctx.tee.as_ref().and_then(Tee::arm);
        let shared = Arc::new(Shared {
            state: Mutex::new(ReaderState {
                chunks: VecDeque::new(),
                fetched_until: 0,
                fetch_from: 0,
                consumed_until: 0,
                epoch: 0,
                eof: false,
                error: None,
                closed: false,
                producer_done: false,
                tee,
            }),
            data_ready: Condvar::new(),
            producer_ctl: Notify::new(),
        });
        runtime.spawn(producer_task(Arc::clone(&shared), Arc::clone(&ctx)));
        Ok(Self {
            ctx,
            shared,
            pos: 0,
            runtime,
        })
    }

    /// 生产者退出观测（测试用）：reader drop 后轮询退出标志。
    #[cfg(test)]
    pub(crate) fn producer_watch(&self) -> ProducerWatch {
        ProducerWatch {
            shared: Arc::clone(&self.shared),
        }
    }
}

/// 生产者退出观测句柄（测试用）。
#[cfg(test)]
pub(crate) struct ProducerWatch {
    shared: Arc<Shared>,
}

#[cfg(test)]
impl ProducerWatch {
    pub(crate) fn exited(&self) -> bool {
        self.shared.state.lock().unwrap().producer_done
    }
}

impl Read for DecryptReader {
    fn read(&mut self, out_buf: &mut [u8]) -> io::Result<usize> {
        if out_buf.is_empty() {
            return Ok(0);
        }
        let audio_len = self.ctx.audio_len;
        let retain = self.ctx.params.retain;
        let prefetch = self.ctx.params.prefetch;
        let mut st = self.shared.state.lock().unwrap();
        loop {
            // 1. 流末尾（Seek 越界 clamp 到 len，pos 不会超过 audio_len）
            if self.pos >= audio_len {
                return Ok(0);
            }
            // 2. 找到覆盖 pos 的分块（队首可能保留回看数据）→ 拷贝返回
            let covering = st
                .chunks
                .iter()
                .position(|c| c.offset <= self.pos && self.pos < c.end());
            if let Some(i) = covering {
                let (n, new_pos) = {
                    let chunk = st.chunks.get_mut(i).unwrap();
                    let skip = (self.pos - chunk.offset) as usize;
                    let avail = chunk.bytes.len() - skip;
                    let n = avail.min(out_buf.len());
                    out_buf[..n].copy_from_slice(&chunk.bytes[skip..skip + n]);
                    (n, self.pos + n as u64)
                };
                self.pos = new_pos;
                st.consumed_until = new_pos;
                // 回看保留修剪：队首只保留 pos 之后 retain 字节。注意
                // 完全消费的分块也靠本循环淘汰（retain=0 即立即丢弃），
                // 否则回看窗口会失效；末 chunk 因 end > pos - retain 不会
                // 被修剪掉，eof 前队列非空的不变量由此保持
                while st
                    .chunks
                    .front()
                    .is_some_and(|c| c.end() + retain <= self.pos)
                {
                    st.chunks.pop_front();
                }
                let resume = st.fetched_until.saturating_sub(self.pos) < prefetch;
                drop(st);
                if resume {
                    // 预取暂停可能解除，唤醒生产者重估
                    self.shared.producer_ctl.notify_one();
                }
                return Ok(n);
            }
            // 2b. 防御：分块按序连续，pos 未被覆盖而前方已有更晚分块 =
            //     内部状态损坏（不应发生），显式报错而非死等
            if st.chunks.iter().any(|c| c.offset > self.pos) {
                drop(st);
                return Err(io::Error::other("internal: buffered hole at read pos"));
            }
            // 3. 粘滞错误（缓冲数据已在上一步优先投递）
            if let Some(e) = st.error.as_ref() {
                let e = io::Error::new(e.kind(), e.to_string());
                drop(st);
                return Err(e);
            }
            // 4. eof：不变量下不可达（pos < audio_len 时末 chunk 必在队列），
            //    防御性报错而非静默截断
            if st.eof {
                drop(st);
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "stream ended before audio_len",
                ));
            }
            // 5. 数据饥饿：等待生产者 push（wait 释放锁，被 notify_all 唤醒重查）。
            //    pos 已达 fetched_until（如 seek 恰落取流头）而生产者 park
            //    在 Notify 上时，等价于数据永不到来——先兜底唤醒重估
            //    （reader_starved_read_notifies_parked_producer）。
            if self.pos >= st.fetched_until && !st.eof {
                self.shared.producer_ctl.notify_one();
            }
            st = self
                .shared
                .data_ready
                .wait(st)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

impl Seek for DecryptReader {
    fn seek(&mut self, seek: SeekFrom) -> io::Result<u64> {
        let audio_len = self.ctx.audio_len;
        let base: i128 = match seek {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::End(d) => audio_len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        // 越界 clamp 到 [0, len]（对齐 std::fs::File::seek 的宽容语义；
        // pos == len 时 Read 直接 Ok(0)）
        let clamped = base.clamp(0, audio_len as i128) as u64;

        {
            let mut st = self.shared.state.lock().unwrap();
            // 缓冲窗口 = [队首 offset, fetched_until]；队首可能保留回看
            // 数据（修剪策略见 Read），队列空时退化为点窗口 {fetched_until}
            // （此前数据已按保留上限修剪，仅该点可即时续读）。
            // pos == audio_len 时永不需要新数据，视同窗口内。
            let window_start = st.chunks.front().map_or(st.fetched_until, |c| c.offset);
            let in_window =
                clamped == audio_len || (window_start <= clamped && clamped <= st.fetched_until);
            if !in_window {
                // 窗口外：epoch+1（生产者据此丢弃在飞请求）、清 chunks、
                // eof/error 复位，通知生产者在目标偏移重启
                st.epoch += 1;
                st.chunks.clear();
                st.fetched_until = clamped;
                st.fetch_from = clamped;
                st.consumed_until = clamped;
                st.eof = false;
                st.error = None;
                drop(st);
                self.shared.producer_ctl.notify_one();
            } else if clamped > st.consumed_until {
                // 窗口内前向 seek：旧位置之前的缓冲作废，消费位置单调推进
                // ——生产者的暂停判定（fetched_until - consumed_until >=
                // prefetch）可能就此解除。不唤醒则 park 中的生产者无人叫醒，
                // read 在 fetched_until 处饥饿时双向死锁
                // （reader_in_window_seek_to_park_head_no_hang）。
                st.consumed_until = clamped;
                drop(st);
                self.shared.producer_ctl.notify_one();
            }
        }
        self.pos = clamped;
        Ok(clamped)
    }
}

impl Drop for DecryptReader {
    /// reader 丢弃 = 换曲/停止（等价原代理的 cancel_on_drop）：
    /// 置 closed 让生产者退出；tee 缓存收尾（完整）或后台补齐（缺尾）。
    fn drop(&mut self) {
        {
            let mut st = self.shared.state.lock().unwrap();
            st.closed = true;
        }
        self.shared.producer_ctl.notify_one();
        let tee = self.shared.state.lock().unwrap().tee.take();
        if let Some(tee) = tee {
            finish_tee(&self.runtime, &self.ctx, tee);
        }
    }
}

// ── 生产者 ───────────────────────────────────────────────────────────

/// 后台生产任务：顺序预取 + 分块解密 + tee 回写 + 入队。
async fn producer_task(shared: Arc<Shared>, ctx: Arc<SourceContext>) {
    let audio_len = ctx.audio_len;
    let params = ctx.params;

    let (mut my_epoch, mut next_fetch) = {
        let st = shared.state.lock().unwrap();
        (st.epoch, st.fetch_from)
    };

    'main: loop {
        // ── 状态同步：drop 退出；窗口外 seek 换起点 ─────────────────
        {
            let st = shared.state.lock().unwrap();
            if st.closed {
                break;
            }
            if st.epoch != my_epoch {
                my_epoch = st.epoch;
                next_fetch = st.fetch_from;
            }
        }

        // ── 本代已拉满：置 eof 后待命（seek 可再唤醒） ──────────────
        if next_fetch >= audio_len {
            // 块作用域强制所有路径上锁在 await 前释放（不变量 ①）
            let current = {
                let mut st = shared.state.lock().unwrap();
                match (st.closed, st.epoch == my_epoch) {
                    (true, _) => None,
                    (false, ok) => {
                        if ok {
                            st.eof = true;
                        }
                        Some(ok)
                    }
                }
            };
            match current {
                None => break,
                Some(true) => shared.data_ready.notify_all(),
                Some(false) => continue 'main,
            }
            match park_for_directive(&shared, my_epoch).await {
                Some((epoch, fetch_from)) => {
                    my_epoch = epoch;
                    next_fetch = fetch_from;
                }
                None => break,
            }
            continue 'main;
        }

        // ── 预取暂停：未读量达上限 ─────────────────────────────────
        let paused = {
            let st = shared.state.lock().unwrap();
            if st.closed {
                break;
            }
            if st.epoch != my_epoch {
                continue 'main;
            }
            st.fetched_until.saturating_sub(st.consumed_until) >= params.prefetch
        };
        if paused {
            // 等待消费进度 / seek / drop 唤醒后整体重估
            shared.producer_ctl.notified().await;
            continue 'main;
        }

        // ── 拉取一个窗口 ───────────────────────────────────────────
        let end = (next_fetch + params.chunk - 1).min(audio_len - 1);
        let range = ByteRange {
            start: next_fetch,
            end,
        };
        let fetched = fetch_with_cancel(
            &shared,
            my_epoch,
            fetch_and_decrypt_range(
                ctx.client.clone(),
                ctx.cdn_url.clone(),
                Arc::clone(&ctx.cipher),
                Arc::clone(&ctx.sem),
                range,
                ctx.total_len,
            ),
        )
        .await;

        let mut stream: OwnedChunkStream = match fetched {
            // closed / epoch 变化：请求已随 future drop 取消。注意不能
            // 在此退出——epoch 变化（窗口外 seek）必须回到 'main 顶部
            // 同步新起点重启；仅 closed 时在顶部退出
            None => continue 'main,
            Some(Err(e)) => {
                if report_error(&shared, my_epoch, e) {
                    shared.data_ready.notify_all();
                }
                match park_for_directive(&shared, my_epoch).await {
                    Some((epoch, fetch_from)) => {
                        my_epoch = epoch;
                        next_fetch = fetch_from;
                    }
                    None => break,
                }
                continue 'main;
            }
            Some(Ok(stream)) => stream,
        };

        // ── 消费流：逐 chunk tee + 入队 ────────────────────────────
        let mut offset = next_fetch;
        loop {
            tokio::select! {
                item = stream.next() => {
                    match item {
                        Some(Ok(bytes)) => {
                            let chunk_off = offset;
                            offset += bytes.len() as u64;
                            // 锁内复查 epoch：seek 竞态下丢弃陈旧分块
                            let mut st = shared.state.lock().unwrap();
                            if st.closed || st.epoch != my_epoch {
                                continue 'main;
                            }
                            if let Some(tee) = st.tee.as_mut() {
                                tee_on_chunk(tee, chunk_off, &bytes);
                            }
                            st.push_chunk(chunk_off, bytes);
                            drop(st);
                            shared.data_ready.notify_all();
                        }
                        Some(Err(e)) => {
                            if report_error(&shared, my_epoch, e) {
                                shared.data_ready.notify_all();
                            }
                            match park_for_directive(&shared, my_epoch).await {
                                Some((epoch, fetch_from)) => {
                                    my_epoch = epoch;
                                    next_fetch = fetch_from;
                                }
                                None => break 'main,
                            }
                            continue 'main;
                        }
                        None => {
                            // 窗口完整交付（流自带 delivered==expected 校验）
                            next_fetch = offset;
                            break;
                        }
                    }
                }
                _ = shared.producer_ctl.notified() => {
                    let abort = {
                        let st = shared.state.lock().unwrap();
                        st.closed || st.epoch != my_epoch
                    };
                    if abort {
                        // drop stream → reqwest 在飞请求取消
                        continue 'main;
                    }
                    // 假唤醒（消费进度通知）→ 继续流式
                }
            }
        }
    }

    // ── 退出：标记完成并唤醒等待中的消费者 ─────────────────────────
    {
        let mut st = shared.state.lock().unwrap();
        st.producer_done = true;
    }
    shared.data_ready.notify_all();
}

/// 错误置入共享状态（粘滞至窗口外 seek）。返回是否生效（epoch 匹配且未关闭）。
fn report_error(shared: &Shared, my_epoch: u64, e: io::Error) -> bool {
    let mut st = shared.state.lock().unwrap();
    if st.closed || st.epoch != my_epoch {
        return false;
    }
    st.error = Some(e);
    true
}

/// 生产者待命：直到窗口外 seek（返回新 `(epoch, 起点)`）或 drop（`None`）。
async fn park_for_directive(shared: &Shared, my_epoch: u64) -> Option<(u64, u64)> {
    loop {
        shared.producer_ctl.notified().await;
        let st = shared.state.lock().unwrap();
        if st.closed {
            return None;
        }
        if st.epoch != my_epoch {
            return Some((st.epoch, st.fetch_from));
        }
        // 假唤醒（消费进度）→ 继续待命
    }
}

/// 在可取消等待中驱动取流 future：closed / 窗口外 seek 时丢弃底层请求
/// （reqwest 响应流随 future drop 取消）；消费进度等假唤醒原样放行。
///
/// `futures_util::future::select` 要求 `Unpin`，故先装箱；输掉的一方
/// 原样归还，假唤醒不会造成重复请求。
async fn fetch_with_cancel<T, F>(shared: &Shared, my_epoch: u64, fut: F) -> Option<T>
where
    F: Future<Output = T> + Send,
{
    // Unpin 化（select 约束）
    let mut fut: std::pin::Pin<Box<dyn Future<Output = T> + Send>> = Box::pin(fut);
    loop {
        // Notified 同样 !Unpin，装箱满足 select 约束
        let ctl = Box::pin(shared.producer_ctl.notified());
        match select(fut, ctl).await {
            Either::Left((out, _ctl)) => return Some(out),
            Either::Right(((), fut_back)) => {
                fut = fut_back;
                let st = shared.state.lock().unwrap();
                if st.closed || st.epoch != my_epoch {
                    return None;
                }
            }
        }
    }
}

// ── tee 边播边缓存 ───────────────────────────────────────────────────

/// tee 写句柄状态（挂在 `ReaderState` 上，随生产者锁段访问）。
struct Tee {
    root: PathBuf,
    key: String,
    tmp: PathBuf,
    /// 追加写句柄；detach / 收尾后为 `None`。
    writer: Option<BufWriter<std::fs::File>>,
    /// 已连续写入的明文长度（tmp 内容 == 明文 `[0, next_write)`）。
    next_write: u64,
    /// 文件头魔数累积（≤ 8 字节，用于 `extension_from_magic`）。
    head: Vec<u8>,
    /// 首个 teed chunk 推断的扩展名。
    ext: Option<&'static str>,
    /// 写失败污染标记（tmp 已删；收尾只清去重键）。
    poisoned: bool,
}

impl Tee {
    /// 武装 tee：final 已存在或同键回填进行中（INFLIGHT 去重）→ 不武装。
    fn arm(plan: &TeePlan) -> Option<Tee> {
        if crate::decrypt::lookup_valid(&plan.root, &plan.key).is_some() {
            debug!(key = %plan.key, "tee: final 缓存已存在，不武装");
            return None;
        }
        if !crate::inflight_insert(plan.key.clone()) {
            debug!(key = %plan.key, "tee: 同键回填进行中，不武装");
            return None;
        }
        let tmp = tmp_path(&plan.root, &plan.key);
        match std::fs::File::create(&tmp) {
            Ok(f) => Some(Tee {
                root: plan.root.clone(),
                key: plan.key.clone(),
                tmp,
                writer: Some(BufWriter::new(f)),
                next_write: 0,
                head: Vec::new(),
                ext: None,
                poisoned: false,
            }),
            Err(e) => {
                warn!(key = %plan.key, %e, "tee tmp 创建失败，不武装");
                crate::inflight_remove(&plan.key);
                None
            }
        }
    }

    /// 永久 detach：冲刷并释放写句柄（tmp 保留，drop 后由补齐任务收尾）。
    fn detach(&mut self) {
        if let Some(mut w) = self.writer.take() {
            let _ = w.flush();
        }
    }
}

/// 生产者每产出明文 chunk 的 tee 回写（生产者锁段内调用）。
///
/// 写入采用 std `File` + `BufWriter` 的顺序小写：仅进 OS 页缓存（无
/// fsync）、append-only，每 chunk 微秒级；且消费者绝不等待 tee（tee 在
/// 生产者侧，`read` 只等数据到达）——缓存写慢最坏拖慢预取，不阻塞播放。
fn tee_on_chunk(tee: &mut Tee, offset: u64, bytes: &[u8]) {
    if tee.poisoned || tee.writer.is_none() {
        return; // 已 detach / 污染：本 reader 不再写
    }
    if offset < tee.next_write {
        // 探测期回读（窗口外 seek 回拉已写过区间）：跳过，不 detach，
        // 生产者追上 next_write 后继续追加
        return;
    }
    if offset > tee.next_write {
        // 前向 seek 跳洞：区间不再连续，永久 detach（tmp 保留，drop 后
        // 由补齐任务单区间拉满 [next_write, audio_len)）
        warn!(
            key = %tee.key, offset, next_write = tee.next_write,
            "tee 缓存遇跳洞，永久 detach"
        );
        tee.detach();
        return;
    }
    if tee.ext.is_none() && tee.head.len() < 8 {
        let want = (8 - tee.head.len()).min(bytes.len());
        tee.head.extend_from_slice(&bytes[..want]);
        if tee.head.len() == 8 {
            tee.ext = extension_from_magic(&tee.head);
        }
    }
    match tee.writer.as_mut().unwrap().write_all(bytes) {
        Ok(()) => tee.next_write += bytes.len() as u64,
        Err(e) => {
            warn!(key = %tee.key, %e, "tee 缓存写失败，detach 并丢弃");
            tee.poisoned = true;
            tee.writer = None;
            let _ = std::fs::remove_file(&tee.tmp);
        }
    }
}

/// reader drop 时的 tee 收尾：完整 → 改名 final + 驱逐；缺尾 → 后台补齐。
fn finish_tee(runtime: &tokio::runtime::Handle, ctx: &Arc<SourceContext>, mut tee: Tee) {
    if tee.poisoned {
        // 写失败路径已删 tmp 并告警；只需释放去重键
        crate::inflight_remove(&tee.key);
        return;
    }
    if let Some(w) = tee.writer.as_mut() {
        let _ = w.flush();
    }
    tee.writer = None;

    if tee.next_write >= ctx.audio_len {
        // 读穿全曲：tmp 已含完整明文 → 收尾（后台执行，drop 保持轻量）
        let root = tee.root.clone();
        let key = tee.key.clone();
        let tmp = tee.tmp.clone();
        let ext = tee.ext;
        let spawned = spawn_detached(runtime, async move {
            finalize_cache_file(&root, &key, &tmp, ext);
        });
        if !spawned {
            // runtime 已关闭（进程退出路径）：同步收尾尽力而为
            finalize_cache_file(&tee.root, &tee.key, &tee.tmp, tee.ext);
        }
    } else {
        // 中途换曲/停止：后台补齐 [next_write, audio_len) 单区间后收尾
        let (key, tmp) = (tee.key.clone(), tee.tmp.clone());
        let spawned = spawn_detached(runtime, complete_tee(Arc::clone(ctx), tee));
        if !spawned {
            warn!(key = %key, "tee 补齐任务无法启动（runtime 已关闭），丢弃");
            let _ = std::fs::remove_file(&tmp);
            crate::inflight_remove(&key);
        }
    }
}

/// 后台补齐：复用 [`fetch_and_decrypt_range`] 续拉 `[next_write, audio_len)`
/// 单区间追加写完 → 收尾。失败丢弃 tmp（仅告警，不影响播放）。
async fn complete_tee(ctx: Arc<SourceContext>, tee: Tee) {
    let result: io::Result<()> = async {
        if tee.next_write < ctx.audio_len {
            let range = ByteRange {
                start: tee.next_write,
                end: ctx.audio_len - 1,
            };
            let mut stream = fetch_and_decrypt_range(
                ctx.client.clone(),
                ctx.cdn_url.clone(),
                Arc::clone(&ctx.cipher),
                Arc::clone(&ctx.sem),
                range,
                ctx.total_len,
            )
            .await?;
            // std File + BufWriter：与 tee 主路径同一理由（页缓存顺序小写）
            let mut w = BufWriter::new(std::fs::OpenOptions::new().append(true).open(&tee.tmp)?);
            while let Some(item) = stream.next().await {
                let bytes = item?;
                w.write_all(&bytes)?;
            }
            w.flush()?;
        }
        Ok(())
    }
    .await;
    let ext = tee.ext;
    match result {
        Ok(()) => finalize_cache_file(&tee.root, &tee.key, &tee.tmp, ext),
        Err(e) => {
            warn!(key = %tee.key, %e, "tee 缓存补齐失败，丢弃");
            let _ = std::fs::remove_file(&tee.tmp);
            crate::inflight_remove(&tee.key);
        }
    }
}

/// 收尾：魔数 → 扩展名 → 原子 rename → 容量驱逐 → 释放去重键。
/// 魔数无法识别或改名失败均丢弃 tmp（缓存键空间与回退/回填路径一致，
/// 二次播放 [`crate::cached_playable_uri`] 命中语义不变）。
fn finalize_cache_file(root: &Path, key: &str, tmp: &Path, ext_hint: Option<&'static str>) {
    let ext = ext_hint.or_else(|| {
        let head = crate::decrypt::read_first_bytes(tmp, 8).unwrap_or_default();
        extension_from_magic(&head)
    });
    match ext {
        Some(ext) => match std::fs::rename(tmp, final_path(root, key, ext)) {
            Ok(()) => {
                debug!(key, ext, "tee 缓存收尾完成");
                if let Err(e) = cache::evict_if_needed(root) {
                    warn!(%e, "cache eviction failed");
                }
            }
            Err(e) => {
                warn!(key, %e, "tee 缓存改名失败，丢弃");
                let _ = std::fs::remove_file(tmp);
            }
        },
        None => {
            warn!(key, "tee 缓存魔数无法识别，丢弃");
            let _ = std::fs::remove_file(tmp);
        }
    }
    crate::inflight_remove(key);
}

/// 在 runtime 上 spawn 分离任务；runtime 已关闭（进程退出）时返回 false。
fn spawn_detached<F>(runtime: &tokio::runtime::Handle, fut: F) -> bool
where
    F: Future<Output = ()> + Send + 'static,
{
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.spawn(fut);
    }))
    .is_ok()
}

// ── 测试 ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    use std::time::Duration;

    use wiremock::matchers::{header_exists, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 窗口参数：chunk=64 / prefetch=2*chunk-1。prefetch 取 127 使生产者
    /// 的 park 头与探测读的落地时机（consumed ∈ {0,1}）无关：
    /// 128 - consumed >= 127 对两者均成立 → 确定性 park 在 2*chunk。
    const CHUNK: u64 = 64;
    const PREFETCH: u64 = 2 * CHUNK - 1;
    /// 唯一可能的 park 头（暂停判定首个成立的窗口边界）。
    const PARK_HEAD: u64 = 2 * CHUNK;
    /// audio_len 远大于 park 头，生产者先到暂停 park 而非 eof 待命态。
    const AUDIO_LEN: u64 = 4096;
    /// 挂起检测超时：修复前 read 永久阻塞，超时即回归实锤。
    const HANG_TIMEOUT: Duration = Duration::from_secs(10);

    /// 明文直通密码（tee=None 场景，测试不触碰缓存环境）。
    struct PlainCipher;

    impl Qmc2Cipher for PlainCipher {
        fn decrypt(&self, _offset: usize, _buf: &mut [u8]) {}
    }

    fn plaintext() -> Vec<u8> {
        (0..AUDIO_LEN).map(|i| (i % 251) as u8).collect()
    }

    fn window_params() -> ReaderParams {
        ReaderParams {
            chunk: CHUNK,
            prefetch: PREFETCH,
            retain: CHUNK,
        }
    }

    /// 明文 Range CDN mock：GET+Range → 206 严格 Content-Range。
    /// 直构 `SourceContext`（不走 prepare_media 探测），只需 GET mock。
    /// 返回 server 本体由测试持有至结束——只返回 uri 的话 MockServer 在
    /// 函数出口即析构，wiremock 异步拆除监听后端口会被全量并发跑的
    /// 其他测试服务器复用，reader 的后续拉取命中别家 mock（表现为
    /// Content-Range 错配/预取停摆）。
    async fn setup_plain_cdn(body: Vec<u8>) -> (MockServer, String) {
        let total = body.len() as u64;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header_exists("Range"))
            .respond_with(move |req: &wiremock::Request| {
                let range_val = req
                    .headers
                    .get("Range")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                let Some(spec) = range_val.strip_prefix("bytes=") else {
                    return ResponseTemplate::new(416);
                };
                let Some((s, e)) = spec.split_once('-') else {
                    return ResponseTemplate::new(416);
                };
                let (Ok(s), Ok(e)) = (s.parse::<u64>(), e.parse::<u64>()) else {
                    return ResponseTemplate::new(416);
                };
                if s >= total {
                    return ResponseTemplate::new(416);
                }
                let end = e.min(total - 1);
                ResponseTemplate::new(206)
                    .insert_header("Content-Range", format!("bytes {s}-{end}/{total}"))
                    .set_body_bytes(body[s as usize..=end as usize].to_vec())
            })
            .mount(&server)
            .await;
        let uri = server.uri();
        (server, uri)
    }

    fn plain_ctx(url: String, params: ReaderParams) -> Arc<SourceContext> {
        Arc::new(SourceContext {
            // no_proxy：本机系统代理（Clash）会把发往 mock 的回环请求也劫持，
            // 高并发下间歇性篡改 206 响应——与生产 cdn_client() 语义对齐
            client: crate::stream::source::cdn_client(),
            cdn_url: url,
            cipher: Arc::new(PlainCipher),
            audio_len: AUDIO_LEN,
            total_len: AUDIO_LEN,
            sem: Arc::new(Semaphore::new(4)),
            tee: None,
            params,
        })
    }

    /// 打开 reader 并读 1 字节（blocking 线程：read 饥饿时阻塞 condvar，
    /// 不能占住测试 runtime 线程饿死生产者任务）。
    async fn open_and_read_one(reader: DecryptReader) -> (DecryptReader, u8) {
        testutil::blocking(move || {
            let mut reader = reader;
            let mut buf = [0u8; 1];
            reader.read_exact(&mut buf).unwrap();
            (reader, buf[0])
        })
        .await
    }

    /// 等待生产者 park 在 `PARK_HEAD`（暂停判定唯一可能的首个成立点）：
    /// fetched_until 到达后短暂静置，确认不再推进（未 park 则会续拉）。
    async fn wait_parked(reader: &DecryptReader) {
        testutil::eventually("生产者到达 park 头", || {
            let fetched = reader.shared.state.lock().unwrap().fetched_until;
            (fetched == PARK_HEAD).then_some(())
        })
        .await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let fetched = reader.shared.state.lock().unwrap().fetched_until;
        assert_eq!(fetched, PARK_HEAD, "生产者未在预期位置 park");
    }

    /// 限时单字节 read（独立 std::thread 执行；挂起时线程随进程回收）。
    /// 不得用 spawn_blocking：泄漏的 blocking 线程会让 `#[tokio::test]`
    /// 的 runtime drop 永久等待，超时保护反而变成套件级死锁。
    async fn read_one_with_timeout(
        reader: DecryptReader,
        hang_msg: &'static str,
    ) -> (DecryptReader, u8) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 1];
            let byte = reader.read(&mut buf).map(|_| buf[0]);
            let _ = tx.send((reader, byte));
        });
        let (reader, byte) = tokio::time::timeout(HANG_TIMEOUT, rx)
            .await
            .expect(hang_msg)
            .expect("read thread dropped the result channel");
        (reader, byte.expect("read failed"))
    }

    /// 回归（审计实锤的永久挂起）：窗口内 seek 恰落在 fetched_until
    /// （生产者 park 的取流头）后，read 无覆盖分块 → 在 data_ready 上
    /// 永久等待；生产者的 park 判定（fetched_until - consumed_until >=
    /// prefetch）基于推进前的 consumed_until 仍成立，park 在 Notify 上
    /// 无人唤醒 → 双向永久挂起。修复：Seek 的 in-window 分支单调推进
    /// consumed_until 并 notify_one，判定立即失真解除、续拉数据。
    #[tokio::test]
    async fn reader_in_window_seek_to_park_head_no_hang() {
        let (_server, url) = setup_plain_cdn(plaintext()).await;
        let reader = DecryptReader::spawn(plain_ctx(url, window_params())).unwrap();

        // 读 1 字节（consumed=1），此后不再读，让生产者推到 park 头
        let (mut reader, first) = open_and_read_one(reader).await;
        assert_eq!(first, 0);
        wait_parked(&reader).await;

        // 窗口内（[0, 128]）seek 恰到 park 头 == fetched_until
        reader.seek(SeekFrom::Start(PARK_HEAD)).unwrap();

        // 修复前：read 永久挂起 → 本断言超时失败；修复后：seek 的 notify
        // 解除生产者 park、续拉 [128, 192)，read 返回该字节
        let (reader, byte) = read_one_with_timeout(
            reader,
            "回归：窗口内 seek 到 fetched_until（park 头）后 read 永久挂起",
        )
        .await;
        assert_eq!(byte, 128);

        // 生产者判定解除后续拉至下一 park 点：consumed=129，
        // 256 - 129 = 127 >= prefetch → park 在 4*chunk
        testutil::eventually("seek 后生产者续拉至下一 park 点", || {
            let fetched = reader.shared.state.lock().unwrap().fetched_until;
            (fetched >= PARK_HEAD + 2 * CHUNK).then_some(())
        })
        .await;
        drop(reader);
    }

    /// 窗口内前向 seek 到缓冲窗口内部（非队首）位置：consumed_until 推进
    /// 须立即解除生产者的 park 判定——无需任何后续 read，预取即恢复。
    /// 修复前：判定基于推进前的 consumed_until（=1）仍成立，无 read 则
    /// fetched_until 永久冻结在 park 头。
    #[tokio::test]
    async fn reader_in_window_interior_seek_resumes_prefetch() {
        let (_server, url) = setup_plain_cdn(plaintext()).await;
        let reader = DecryptReader::spawn(plain_ctx(url, window_params())).unwrap();

        let (mut reader, first) = open_and_read_one(reader).await;
        assert_eq!(first, 0);
        wait_parked(&reader).await;

        // 窗口内部目标：consumed(1) < 64 < park 头(128)，且
        // 128 - 64 = 64 < prefetch → seek 后判定必解除
        let target = PARK_HEAD - CHUNK;
        reader.seek(SeekFrom::Start(target)).unwrap();

        // 不做任何 read：生产者须自行续拉一个窗口（128 → 192）
        testutil::eventually("窗口内 seek 后预取自行恢复", || {
            let fetched = reader.shared.state.lock().unwrap().fetched_until;
            (fetched >= PARK_HEAD + CHUNK).then_some(())
        })
        .await;

        // 目标仍在缓冲窗口内：内容即时可读且正确
        let (reader, byte) = read_one_with_timeout(reader, "缓冲内 read 不应挂起").await;
        assert_eq!(byte, target as u8);
        drop(reader);
    }

    /// 修复 2（兜底唤醒）单测：白盒构造「暂停判定已失真解除但生产者仍
    /// park 在 Notify 上」的漏网状态（模拟某条推进 consumed_until 却未
    /// 通知的路径），read 饥饿（pos == fetched_until）前必须 notify 唤醒
    /// 生产者续拉，而非永久等待。
    #[tokio::test]
    async fn reader_starved_read_notifies_parked_producer() {
        let (_server, url) = setup_plain_cdn(plaintext()).await;
        let reader = DecryptReader::spawn(plain_ctx(url, window_params())).unwrap();

        let (mut reader, first) = open_and_read_one(reader).await;
        assert_eq!(first, 0);
        wait_parked(&reader).await;

        // 状态手术：consumed_until 推进到判定解除（128 - 2 = 126 < 127）
        // 但不经过任何 notify 路径；pos 直置 fetched_until（无覆盖分块
        // 的饥饿点）
        {
            let mut st = reader.shared.state.lock().unwrap();
            st.consumed_until = PARK_HEAD + 1 - PREFETCH;
        }
        reader.pos = PARK_HEAD;

        // 修复前：无 notify → 生产者 park 不动 → read 永久挂起；修复后：
        // 饥饿分支兜底 notify_one → 生产者重估（判定已解除）→ 续拉
        // [128, 192) → read 返回
        let (reader, byte) =
            read_one_with_timeout(reader, "回归：read 饥饿前未兜底唤醒 park 中的生产者").await;
        assert_eq!(byte, PARK_HEAD as u8 % 251);
        drop(reader);
    }
}
