# Slint 1.19 里与本仓相关的变更（摘录）

出处：<https://github.com/slint-ui/slint/blob/master/CHANGELOG.md> 的 `1.19.0`（2026-11-16）与 `1.19.1`（2026-11-24）两节。升级自 1.18.1，对比截图见同目录 PNG。

## 直接影响本仓的

- **Breaking**：实验特性必须在 `SLINT_ENABLE_EXPERIMENTAL_FEATURES` 里点名，`1` 与未知名字都是错误 → `crates/hmp-desktop/build.rs`。
- **Rust**：最低支持的 Rust 版本变为 1.95 → 工作区 MSRV、`clippy.toml`、CI msrv job。
- `Rectangle`：投影跟随填充与边框的透明度 → 播放页封面那层矩形没有背景，`drop-shadow` 不生效，补 `background`。
- 布局：横向子元素的 `x` / `y` 不再被布局的 padding 覆盖 → 队列行容器上多余的 `x: 0` 删掉。
- 编译器新警告：「Set a 'background' or border to cast a drop shadow」「The layout stretches the 'width' of this element, so 'x' can move it past the edge」——就是上面两处。

## 解释截图差异的

- 软件渲染器：新增投影支持；`Clip` 跟随 `border-radius`；圆角抗锯齿改进。离屏截图用的是软件渲染器，所以封面的圆角与投影、面板投影、细边框在「升级后」才出现——1.18 的软件渲染器根本不画投影。
- 文本：修了 Skia / FemtoVG 渲染器下对齐文字的抖动、FemtoVG 下部分对齐文字偏一像素。对比图里所有文字边缘的像素差来自这类变化，位置没动。
- 布局：修了重复元素在布局里的若干问题，改进了大量 / 折行 `Text` 的布局性能——带翻译的歌词行在 1.18 里有的翻译没渲染出来，升级后正常；播放页标题按设计折成两行（1.18 里没折、直接省略）。

## 没有写进变更说明、靠实测发现的

- 元素带着最终几何被创建时（例如播放页在当前行已定后才挂载），首次布局赋的 `y` / 高度不再触发 `changed` 回调；1.18 里行先落 0 再被布局赋值，会触发。歌词进页直达当前行因此失效，修法见 AUDIT §30.7 与 `crates/hmp-desktop/tests/lyric_follow.rs`。变更说明里只有一条相关的：「`changed` handlers run right after a property is set from `invoke_from_event_loop()` or an async task」。
