# S6 验收沙箱环境包（单一事实源）：所有沙箱进程运行前 dot-source 本文件。
# 隔离三件套：
#   XDG_*_HOME    -> 数据/配置/缓存目录进 worktree 沙箱（hmp-storage xdg.rs 显式覆盖优先于 Known Folders）
#   HMP_IPC_ENDPOINT -> daemon 命名管道隔离（server::socket_path 单函数供 serve/CLI/桌面共用）
# 纪律：凡运行 worktree 构建的 hmp-desktop 衍生进程（含 headless 示例）必须先包裹本包，
#       否则其内部 connect_or_spawn 会把 daemon 拉到默认管道并写用户真库。
$Global:S6Root = "D:\coding\hutao-music-player\.worktrees\s6-verify\verify"
$env:XDG_CONFIG_HOME   = "$Global:S6Root\sandbox\config"
$env:XDG_DATA_HOME     = "$Global:S6Root\sandbox\data"
$env:XDG_CACHE_HOME    = "$Global:S6Root\sandbox\cache"
$env:HMP_IPC_ENDPOINT  = "\\.\pipe\hmp-s6-verify"
$Global:S6Hmp     = "D:\coding\hutao-music-player\.worktrees\s6-verify\target\release\hmp.exe"
$Global:S6Desktop = "D:\coding\hutao-music-player\.worktrees\s6-verify\target\release\hmp-desktop.exe"
$Global:S6ShotMenu = "D:\coding\hutao-music-player\.worktrees\s6-verify\target\release\examples\shot-menu.exe"
