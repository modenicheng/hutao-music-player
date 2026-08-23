# HMP Tauri 桌面控制器

这个应用是 `hmpd` 的桌面控制器，不包含独立播放内核。Vue、原生 tray 和 CLI 都通过 `hmp-control` 连接同一个 daemon；WebView 不创建 `HTMLAudioElement`。

## 生命周期

- 桌面应用和 `hmpd` 都是当前登录会话内的单例。
- 已有 daemon 时直接连接；否则 Tauri 以 `--frontend-owned` 启动 sidecar。
- 关闭主窗口只隐藏到 tray。tray 的「完整退出」先请求 daemon 优雅退出，最多等待 3 秒，再移除 tray 并结束 GUI。
- frontend-owned daemon 的最后一个 GUI lease 消失后保留 30 秒重连窗口；GUI/tray 崩溃且未恢复时自动退出。
- CLI 以 `--autonomous` 拉起的 daemon 不依赖 GUI lease。桌面应用连接它后仍共享同一状态；桌面「完整退出」会按用户意图关闭整个应用及 daemon。

Linux 的 Tauri tray 仍提供完整右键菜单；受底层平台限制，tray 左键点击事件不保证产生。Windows 上左键会恢复、取消最小化并聚焦主窗口。

## Windows 构建

Windows 音频与 SMTC 使用系统接口，无需安装额外媒体 SDK。在仓库根目录执行：

```powershell
cargo build -p hmp-daemon --bin hmpd --release --no-default-features
./apps/hmp-tauri/scripts/stage-sidecar.ps1
Push-Location apps/hmp-tauri
pnpm install --frozen-lockfile
pnpm test
pnpm tauri build
Pop-Location
```

`stage-sidecar.ps1` 会读取 `rustc -vV` 的 host triple，并生成 Tauri 要求的 `src-tauri/binaries/hmpd-<target>.exe`。如果 daemon 尚未构建，脚本会退出非零并打印准确的构建命令，不会放置伪 sidecar。

当前安装包只需捆绑 `hmpd` sidecar。发布前仍应在未安装开发工具的干净 Windows 环境验证安装包。

## 本地开发

在 `apps/hmp-tauri` 目录运行：

```powershell
pnpm tauri dev
```

该命令会构建 debug 版 daemon，并按当前 Rust host triple 暂存 sidecar，再启动 Tauri 和 Vite。

## 开发检查

```powershell
pnpm test
pnpm build
Push-Location src-tauri
cargo test --lib
cargo check --all-targets
Pop-Location
```

配置了 `bundle.externalBin` 后，Tauri 的 Rust 构建也要求目标三元组对应的 sidecar 已经暂存。
