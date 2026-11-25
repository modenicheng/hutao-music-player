# 修复任务书 · 桌面端（hmp-desktop，Slint）

> §30.7 修复轮任务书（F3 · 桌面端）。审计依据：`docs/AUDIT.md` §30.2.3、§30.2.4（D7、D8）、§30.3.3、§30.7、§29；基线见 `docs/verify/2026-11-25-fix-round/baseline.md`。

你在仓库 hutao-music-player（HMP，Rust 写的 QQ 音乐播放器）的一个**独立 git worktree** 里做修复。桌面端是 Slint 前端：只发意图给常驻 daemon（经 IPC），曲库页面直接只读 SQLite 做投影。背景见主检出的审计文档 §30（只读，不要改它）：
`/mnt/d/code-linux/hutao-music-player/docs/AUDIT.md` 的「## §30」一节，重点读 30.2.3 第 3 条、30.2.4（D7、D8）、30.3.3、30.7，以及 §29 的「零号发现」（Slint 1.18 定时器坑）（不必通读全文档）。

## 你的工作树（只在这里改）
`/mnt/d/code-linux/hutao-music-player/.worktrees/fix-desktop`

**文件所有权**（只允许改 `crates/hmp-desktop/` 下这些）：
- `src/player_bridge.rs`、`src/track_covers.rs`、`src/bridge.rs`、`src/library_view.rs`、`src/main.rs`（如确有必要：`src/online_covers.rs`、`src/cover_cache.rs`）
- `ui/now-playing.slint`、`ui/stores.slint`（如确有必要：`ui/app.slint`、`ui/music-library-page.slint`、`ui/recent-page.slint` 之类直接相关的页面文件）
- `crates/hmp-desktop/tests/` 下新增测试文件

**不要碰** `src/menu_bridge.rs`（另一个代理正在改它）、`crates/hmp-core/`、`crates/hmp-daemon/`、`crates/hmp-storage/`——它们由另外两个代理并行修改。其它工作树与主检出（你的工作树之外）不动。若认为所有权之外的文件必须改，停下来在报告里说明。

## 硬性约束
- 不 commit、不建分支、不 stash、不 reset。改动留在工作树里即可（主代理负责合入）。
- 不新增第三方依赖（`tracing-subscriber` 已在依赖里但从未被使用）。
- 不运行任何 `hmp` / `hmp-desktop` 二进制，不 `cargo run`，不跑 examples；不跑 `--ignored` 测试；不读写 `~/.local/share/hmp`、`~/.config/hmp`、`~/.cache/hmp`。**新增测试不得读真实用户库**——用内存库或 `tempfile` + XDG 环境变量隔离（先看 `crates/hmp-desktop/tests/` 里现有测试怎么隔离；`bridge::load_data` 这类会直读真实库的入口不要在新测试里调用）。
- cargo 只跑下面「验证」里的命令，并带 `--locked --offline`。
- 改动要小而准，不做顺手重构；注释与命名跟周围代码一致（中文文档注释、相同的密度）；UI 文案是中文。
- Slint 1.18 的已知坑：`Timer { running: false }` 上首次 `restart()` 是空操作，要写成 `if (t.running) { t.restart(); } else { t.running = true; }`；无头测试后端不会泵跨线程的 `invoke_from_event_loop`。
- shell 是 zsh：glob 要加引号（`--include='*.rs'`）；循环变量别叫 `path`；`echo ===` 要加引号。
- 只读任务书点名的函数及其直接依赖，不要通读仓库。

## 修复项（行号以当前工作树为准，可能有 ±几行偏差）

### 1. [U1] 歌词一次失败后本会话永不重试
位置：`src/player_bridge.rs` 的 `spawn_lyric_fetch`（约 1143-1200）。
现状：函数内 `thread_local! { static REQUESTED: RefCell<HashSet<String>> }`，入口 `REQUESTED.with(|set| set.borrow_mut().insert(id.clone()))`，返回 false 就直接 return；失败路径从不 remove——而该函数随每次状态推送被调用，所以这个 id 整个会话都不会再请求。函数文档写的是「失败不重试，换曲再回来时自然重试」，与实现相反。
要求：失败（请求出错或拿到空页）后允许重试，但要有退避，不能让 10Hz 的状态推送变成请求风暴：按 id 记「下次可重试时刻」，首次失败约 5 秒后可重试，之后翻倍、封顶 60 秒；成功后清除。账本只在 UI 线程读写（回包本来就走 `invoke_from_event_loop`，把「标记失败」放进那个闭包；入口判断也在 UI 线程）。把闸门逻辑抽成纯函数并单测（失败后立刻不允许、过了间隔允许、成功后清除、间隔翻倍与封顶）。同时把过时的文档注释改对。

### 2. [U2] 封面「失败出账 + 指数退避」跑错了线程
位置：`src/player_bridge.rs`（`COVER_RETRY` 约 1321-1325、`REQUESTED` 约 1418-1421、失败分支约 1472-1478）；`src/track_covers.rs`（`REQUESTED` / `account` / `release`，约 26-110）。
现状：这些去重账本是 `thread_local!`，入账在 UI 线程，但失败分支里的 `REQUESTED.with(..remove..)`、`cover_fetch_schedule_retry` 以及 `track_covers.rs` 的 `release(&key)` 都在 tokio 工作线程上执行——操作的是另一个线程的空集合。结果是 UI 线程的账永不消除：一张封面只要失败一次，整个会话都停在占位图；退避也从未生效。`src/online_covers.rs`（约 16-19）已经改成进程级 `Mutex` 账本，那处是对的。
要求：把这几处账本改成进程级（如 `static Mutex<…>`，参照 `online_covers.rs` 的写法），或者把所有读写严格收在 UI 线程——选一种，全仓这几处保持一致。确保失败的 `CoverGet` 会把 key 从「在账」集合移出并记下退避，使下一次状态推送能重试。注意锁的粒度，别在持锁时再进入会加同一把锁的代码。加一条**在旧实现下必然失败**的回归测试：线程 A `account(key)` 返回 true，线程 B `release(key)`，随后再 `account(key)` 应返回 true（账已清）。`player_bridge.rs` 里对应的账本也加同款测试（必要时把账本操作抽成小函数以便测试）。

### 3. [U3] 桌面端没有任何日志
位置：`crates/hmp-desktop/src/main.rs`（`fn main`，约 11 行起）。
现状：全工作区只有 CLI 的 `main.rs` 初始化了 `tracing_subscriber`；桌面端的 `tracing::warn! / error!` 运行时全是空操作，封面、歌词、连接失败连一条痕迹都没有。
要求：在 `main` 最前面初始化 subscriber：`EnvFilter::try_from_default_env()`，取不到就用 `warn`；输出到追加写的日志文件 `hmp_storage::cache_dir().join("logs/desktop.log")`（创建目录；启动时若文件已超过 5 MiB 就轮转成 `desktop.log.1`）；文件打不开就退回 stderr；用 `try_init()` 避免重复初始化 panic；关掉 ANSI 颜色码。不加依赖。轮转判断抽成小函数并单测。

### 4. [U6] 播放条的歌手/专辑链接用展示名导航，QQ 曲目点了就是「未找到」
位置：`src/player_bridge.rs`（约 1101-1112，`player.set_artists(track.artist_names()..)` / `player.set_album(专辑名)`）、`ui/stores.slint` 的 `Player` 全局、`ui/now-playing.slint`（约 520-535，`Nav.go(Route.artist, Player.artists)` / `Nav.go(Route.album, Player.album)`）。
现状：daemon 推来的 `Track` 本来带着 `artists[i].id`（QQ 的 singermid）和 `album.id`（albummid），桥接层只把展示名写进了 UI，于是点击后拿展示名去本地库猜：先整串精确匹配必空，再按「最长包含」兜底，QQ 曲目的歌手直接落到「未找到」。
要求：
- 先读 `crates/hmp-daemon/src/player.rs`（约 300-330）与 `crates/hmp-daemon/src/local.rs`（约 150-170）确认：QQ 曲目这两个 id 是真 mid，本地曲目是哨兵值（如 `"local"`）。
- `Player` 全局增加 `artist-mid` / `album-mid` 两个字符串属性；换曲时与标题、歌手名一起写入（只在它是 QQ mid 时写，本地哨兵或空串一律写空串）。
- `now-playing.slint` 的两个链接：对应 mid 非空时用远端参数形式导航——`crates/hmp-desktop/src/remote_details.rs` 里的 `REMOTE_PREFIX` + mid（先读该文件约 30-60 行确认格式，以及远端歌手页/专辑页对这个参数的要求）；为空时保持现在的展示名导航不变。
- 歌手链接取第一位歌手的 mid（播放条显示的是拼接串，点进去落到主歌手页即可）。
- 加一条单测覆盖「取哪个 mid / 哨兵视为没有」的纯函数。

### 5. [D7] 「最近播放」不随播放更新
位置：`src/bridge.rs`（`apply_route` 约 747 起；`apply_snapshot` 里 recent 部分约 639-647）、`src/library_view.rs` 的 `recent_rows`（约 249 起）。
现状：桌面端只在收到 `LibraryChanged` 时重查；而引擎写播放历史并不触发这个事件，导航到「最近播放」页也不重读——新会话播了歌再点进去，看到的还是启动时的快照。
要求：导航到 `Route.recent` 时在后台重读最近播放（沿用现有的后台刷新套路，读库不要放在 UI 线程），更新 `recent` 模型及 latest/earliest 两个时间戳。**不要**改 `RECENT_LIMIT`（12）与 `format_stamp` 的 UTC 口径——后者是有意与 `hmp history` 保持一致。能低成本加无头测试就加（看 `crates/hmp-desktop/tests/` 现有写法），否则说明原因。

### 6. [D8] 任何库刷新都会重置音乐库的文件夹过滤
位置：`src/bridge.rs`（`apply_snapshot` 约 660-668：`data.set_local_tracks(model(all_local_rows.clone()))`；`on_filter_folder` 处理器约 698-727）。
现状：`apply_snapshot` 无条件把曲目表重置成全部本地曲目，却不重放当前选中的文件夹；而 `selected_folder` 和表头标题只在点击处理器里写。点一次红心、建一次歌单、文件监听落一批库，都会让文件夹行仍高亮、表头仍写「某文件夹 · N 首」，内容却变成全部。
要求：把过滤 + 设表头的逻辑抽成一个函数，点击处理器与 `apply_snapshot` 末尾都调用它（按当前 `selected_folder` 重放）。保持现有交互不变（再点一次取消、越界回「全部音乐」）。给抽出的函数加单测。

## 验证（在你的工作树里跑；依赖已预热，通常是增量编译）
```
cargo fmt --all -- --check
cargo clippy -p hmp-desktop --all-targets --locked --offline -- -D warnings
cargo test -p hmp-desktop --locked --offline
```
三项都必须通过（clippy 若报出你没碰过的文件里的既有问题，写进报告，不要顺手修）。`cargo fmt --all -- --check` 不过就先 `cargo fmt --all` 再复查。改了 `.slint` 的属性/全局后，留意编译期绑定环、未使用属性等 Slint 编译错误。

## 交付报告（中文，≤ 500 字 + 清单）
1. 每个修复项一行：改了什么（`文件:行号`）、新增测试名。
2. 三条验证命令的结果（通过/失败数；桌面端有 lib 单测 + 多个集成测试二进制）。
3. 没做完、拿不准、或你认为是任务书错的地方，明确写出来。
4. 新增文件清单（若有）。
不要写补丁文件，不要写总结文档。

---

## 更正（2026-11-25 合入时补）

- **U1 的前提有误**：`spawn_lyric_fetch` 原本只在换曲时调用，不在每次状态推送；只加入口闸门的话「5 秒后重试」永远不会触发。代理把入口挪到了每次状态推送（词表未就绪或账本有失败记录时才调）。后果：无词曲目每 60 秒左右重拉一次直到换曲，没有终态（daemon 对空词页有 300 秒负缓存，只是本机 IPC，不出网，保留）；换曲重进会重新拉词。
- `cover_cache.rs` 不在代理的必改清单里：存储代理报出「PNG 封面被存成 `.jpg`、读侧按扩展名解码失败」后，主代理在 `cover_cache.rs` 加了按内容识别格式（提交 `0d5c813`）。
- Slint 1.19 升级后另有一处真回归（歌词进页不直达当前行）与两处编译器新警告，见 AUDIT §30.7 末。
