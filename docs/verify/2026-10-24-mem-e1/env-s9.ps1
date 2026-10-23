# S9 E1 复验沙箱环境包（自 S6 env-s6.ps1 只读复制适配：路径/管道名换 s9-mem-fixes）。
# 隔离三件套同源：XDG_*_HOME 进本 worktree 沙箱 + HMP_IPC_ENDPOINT 独立命名管道。
# 纪律：凡运行本 worktree 构建的 hmp-desktop/hmp 衍生进程必须先 dot-source 本包，
#       否则 connect_or_spawn 会把 daemon 拉到默认管道并写用户真库。
$Global:S9Root = "D:\coding\hutao-music-player\.worktrees\s9-mem-fixes\verify"
$env:XDG_CONFIG_HOME   = "$Global:S9Root\sandbox\config"
$env:XDG_DATA_HOME     = "$Global:S9Root\sandbox\data"
$env:XDG_CACHE_HOME    = "$Global:S9Root\sandbox\cache"
$env:HMP_IPC_ENDPOINT  = "\\.\pipe\hmp-s9-verify"
$Global:S9Hmp     = "D:\coding\hutao-music-player\.worktrees\s9-mem-fixes\target\release\hmp.exe"
$Global:S9Desktop = "D:\coding\hutao-music-player\.worktrees\s9-mem-fixes\target\release\hmp-desktop.exe"
