# S10 桌面退出死锁复现/验收编排（PLAN T5，模式浓缩自 docs/verify/2026-10-24-mem-e1/mem_run_e1.ps1）。
# 隔离三件套：XDG_*_HOME 进本臂沙箱 + HMP_IPC_ENDPOINT 独立命名管道（每臂独立管道名，臂间零干扰）。
# 纪律：音量 0（daemon 侧声音门）、PID 精确杀、绝不 taskkill /IM、成僵尸只取证不反复杀。
# 用法（在本 worktree 根）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File verify-s10\repro.ps1 -ArmName base-quit-1 -BinDir base -ExitMode quit
#   powershell ... -ArmName fix-quit-1  -BinDir fix  -ExitMode quit
#   powershell ... -ArmName fix-close-1 -BinDir fix  -ExitMode close
#   powershell ... -ArmName base-sw-1   -BinDir base -ExitMode quit -Software   # SLINT_BACKEND=winit-software 对照
param(
    [Parameter(Mandatory = $true)][string]$ArmName,
    [Parameter(Mandatory = $true)][ValidateSet("base", "fix", "fix2")][string]$BinDir,
    [Parameter(Mandatory = $true)][ValidateSet("quit", "close")][string]$ExitMode,
    [int]$PlaySeconds = 180,
    [switch]$Software
)
$ErrorActionPreference = "Continue"

$Root      = $PSScriptRoot
$Sandbox   = Join-Path $Root "sandbox-$ArmName"
$HmpExe    = Join-Path $Root "bin-$BinDir\hmp.exe"
$DesktopExe = Join-Path $Root "bin-$BinDir\hmp-desktop.exe"
foreach ($p in @($HmpExe, $DesktopExe)) { if (-not (Test-Path $p)) { throw "binary missing: $p" } }

foreach ($d in @("config", "data", "cache", "media", "log")) {
    New-Item -ItemType Directory -Force -Path (Join-Path $Sandbox $d) | Out-Null
}
$Log = Join-Path $Sandbox "log\run.log"
function L([string]$m) {
    $line = "[{0}] {1}" -f (Get-Date -Format "HH:mm:ss"), $m
    Write-Host $line
    [System.IO.File]::AppendAllText($Log, $line + "`r`n")
}
if (Test-Path $Log) { Remove-Item $Log }
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$BudgetSec = 10 * 60   # 单臂总看门狗（纪律：真机实验 ≤10 分钟）

# —— 隔离环境（子进程继承；管道名含臂名，臂间独立）——
$env:XDG_CONFIG_HOME  = Join-Path $Sandbox "config"
$env:XDG_DATA_HOME    = Join-Path $Sandbox "data"
$env:XDG_CACHE_HOME   = Join-Path $Sandbox "cache"
$env:HMP_IPC_ENDPOINT = "\\.\pipe\hmp-s10-$ArmName"

$desktop_pid = 0
$daemon_pid = 0
$verdict = "INCONCLUSIVE"

function Alive([int]$p) { $null -ne (Get-Process -Id $p -ErrorAction SilentlyContinue) }

# —— 近静音 WAV 种子（220Hz @ -52dBFS，音量门之外双保险）——
function Seed-Media {
    $media = Join-Path $Sandbox "media"
    $existing = Get-ChildItem $media -Filter "*.wav" -ErrorAction SilentlyContinue
    if ($existing.Count -ge 10) { return $media }
    $rate = 44100; $amp = 80; $freq = 220; $secs = 8
    $n = $rate * $secs
    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter($ms)
    $bw.Write([System.Text.Encoding]::ASCII.GetBytes("RIFF")); $bw.Write([int32](36 + $n * 2))
    $bw.Write([System.Text.Encoding]::ASCII.GetBytes("WAVE"))
    $bw.Write([System.Text.Encoding]::ASCII.GetBytes("fmt ")); $bw.Write([int32]16)
    $bw.Write([int16]1); $bw.Write([int16]1); $bw.Write([int32]$rate); $bw.Write([int32]($rate * 2))
    $bw.Write([int16]2); $bw.Write([int16]16)
    $bw.Write([System.Text.Encoding]::ASCII.GetBytes("data")); $bw.Write([int32]($n * 2))
    for ($i = 0; $i -lt $n; $i++) {
        $bw.Write([int16]($amp * [Math]::Sin(2 * [Math]::PI * $freq * $i / $rate)))
    }
    $bw.Flush(); $bytes = $ms.ToArray(); $bw.Close()
    $first = Join-Path $media "s10-silence-01.wav"
    [System.IO.File]::WriteAllBytes($first, $bytes)
    for ($k = 2; $k -le 10; $k++) {
        Copy-Item $first (Join-Path $media ("s10-silence-{0:d2}.wav" -f $k))
    }
    return $media
}

try {
    L "=== S10 repro start (arm=$ArmName bin=$BinDir exit=$ExitMode software=$Software) ==="
    # 0) 开工快照：在跑 hmp 进程（只记录，绝不动）
    Get-CimInstance Win32_Process -Filter "Name like 'hmp%'" -ErrorAction SilentlyContinue | ForEach-Object {
        L ("pre-existing: PID {0} {1} PPID {2}" -f $_.ProcessId, $_.Name, $_.ParentProcessId)
    }

    # 1) 旧隔离 daemon 清场（只打本臂隔离端点）
    & $HmpExe quit 2>$null | Out-Null
    Start-Sleep -Seconds 3

    # 2) 起隔离 daemon
    $daemon = Start-Process -FilePath $HmpExe -ArgumentList "serve" -WindowStyle Hidden -PassThru
    $daemon_pid = $daemon.Id
    Start-Sleep -Seconds 4
    if (-not (Alive $daemon_pid)) { throw "daemon failed to start" }
    L "daemon pid=$daemon_pid endpoint=$env:HMP_IPC_ENDPOINT"

    # 3) 声音门 + 指纹
    & $HmpExe volume 0 | Out-Null
    $statusJson = & $HmpExe status --json | ConvertFrom-Json
    if ($statusJson.playback.user_volume -ne 0) { throw "VOLUME GATE FAIL" }
    L "volume gate PASS (user_volume=0) backend_build=$($statusJson.backend_build)"
    $exeH = Get-Item $HmpExe; $exeD = Get-Item $DesktopExe
    L ("exe same-batch: hmp mtime={0}, desktop mtime={1}" -f $exeH.LastWriteTime.ToString("HH:mm:ss"), $exeD.LastWriteTime.ToString("HH:mm:ss"))

    # 4) 种子 + 扫描 + 歌单
    $media = Seed-Media
    & $HmpExe scan $media 2>$null | Out-Null
    L "scan done: $media"
    $created = & $HmpExe playlist create "s10退出" 2>$null
    if ($created -notmatch '#(\d+)') { throw "playlist create failed: $created" }
    $plId = [int]$Matches[1]
    $tracks = Get-ChildItem $media -Filter "*.wav" | ForEach-Object { "local:" + $_.FullName }
    foreach ($t in $tracks) { & $HmpExe playlist add $plId $t 2>$null | Out-Null }
    L "seeded $($tracks.Count) tracks into playlist $plId"

    # 5) 起桌面（software 臂注入 SLINT_BACKEND；其余臂显式清掉防外泄）
    if ($Software) { $env:SLINT_BACKEND = "winit-software" } else { Remove-Item Env:\SLINT_BACKEND -ErrorAction SilentlyContinue }
    $desktop = Start-Process -FilePath $DesktopExe -PassThru
    $desktop_pid = $desktop.Id
    Start-Sleep -Seconds 12
    if (-not (Alive $desktop_pid)) { throw "desktop failed to start" }
    L "desktop pid=$desktop_pid"

    # 6) 播放期：起播 + ~6s 切曲 + 轻队列 churn（浓缩 E1 P1/P2：状态推送驱动队列投影与封面装载）
    $playEnd = $sw.Elapsed.Add([TimeSpan]::FromSeconds($PlaySeconds))
    $rng = New-Object System.Random(20261024)
    $qCount = 0
    while ($sw.Elapsed -lt $playEnd) {
        if ($sw.Elapsed.TotalSeconds -gt $BudgetSec) { L "WATCHDOG TRIP in play phase"; break }
        $st = & $HmpExe status --json 2>$null | ConvertFrom-Json
        if ($st.playback.status -ne "Playing") {
            & $HmpExe play "playlist:local:$plId" 2>$null | Out-Null
        } else {
            & $HmpExe next 2>$null | Out-Null
        }
        if ($qCount -ge 5) { & $HmpExe queue clear 2>$null | Out-Null; $qCount = 0 }
        else { & $HmpExe queue add $tracks[$rng.Next($tracks.Count)] 2>$null | Out-Null; $qCount++ }
        Start-Sleep -Seconds 6
    }
    L "play phase done (${PlaySeconds}s)"

    # 7) GL 后端实测（O3）：桌面进程加载的 GL/驱动模块名
    try {
        $mods = (Get-Process -Id $desktop_pid).Modules |
            Where-Object { $_.ModuleName -match "opengl|EGL|GLES|nvoglv|atio|amd|igd|d3d" } |
            Select-Object -ExpandProperty ModuleName
        L ("GL modules: " + (($mods | Sort-Object -Unique) -join ", "))
    } catch { L "GL module capture failed: $_" }

    # 7.5) 存活门：退出触发前桌面必须在场（真机有用户交互，弹窗被人工关掉
    #      会让 quit 臂假绿——此时判 INCONCLUSIVE 而非 CLEAN_EXIT）
    if (-not (Alive $desktop_pid)) {
        $verdict = "INCONCLUSIVE_DESKTOP_GONE"
        L "desktop gone BEFORE exit trigger (user closed the window? or early exit) -> $verdict"
        L "=== verdict: $verdict (elapsed $([math]::Round($sw.Elapsed.TotalSeconds))s) ==="
        return
    }

    # 8) 退出触发 + 轮询判定（同轮询里记录 daemon 退出时刻——quit 臂断言
    #    daemon 自身 ≤15s 退出，必须在同一时间轴上采样，事后补查会恒假）
    $t0 = $sw.Elapsed
    if ($ExitMode -eq "quit") {
        L "exit trigger: hmp quit (isolated endpoint)"
        & $HmpExe quit 2>$null | Out-Null
    } else {
        L "exit trigger: CloseMainWindow (X close path)"
        $null = (Get-Process -Id $desktop_pid).CloseMainWindow()
    }
    $exitSec = -1
    $daemonExitSec = -1
    while ($sw.Elapsed -lt $t0.Add([TimeSpan]::FromSeconds(60))) {
        if (($daemonExitSec -lt 0) -and ($daemon_pid -ne 0) -and (-not (Alive $daemon_pid))) {
            $daemonExitSec = [math]::Round(($sw.Elapsed - $t0).TotalSeconds, 1)
        }
        if (-not (Alive $desktop_pid)) { $exitSec = [math]::Round(($sw.Elapsed - $t0).TotalSeconds, 1); break }
        Start-Sleep -Seconds 1
    }
    if ($exitSec -ge 0) {
        L "desktop exited after ${exitSec}s"
        $verdict = if ($exitSec -le 5) { "CLEAN_EXIT" } else { "SLOW_EXIT" }
    } else {
        L "desktop still alive at +60s -> Stop-Process -Force"
        Stop-Process -Id $desktop_pid -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 10
        if (Alive $desktop_pid) {
            $verdict = "ZOMBIE"
            L "*** ZOMBIE REPRODUCED (pid=$desktop_pid): force-kill ineffective ***"
            $p = Get-Process -Id $desktop_pid -ErrorAction SilentlyContinue
            if ($p) {
                L ("zombie threads={0} Responding={1} Handles={2}" -f $p.Threads.Count, $p.Responding, $p.HandleCount)
                foreach ($t in $p.Threads) {
                    L ("  TID {0} State={1} WaitReason={2}" -f $t.Id, $t.ThreadState, $t.WaitReason)
                }
            }
            L "zombie left in place (取证留现场，重启才清)"
            $desktop_pid = 0   # 不再尝试清理
        } else {
            $verdict = "FORCE_KILLED"
            L "force-kill worked after hang (not kernel-stuck zombie; user-mode hang?)"
        }
    }

    # 9) daemon 侧断言：退出时刻同轴记录；轮询窗口内未捕获则补等（quit 后
    #    daemon 优雅退出 ~2s，桌面若 1s 先退，轮询会早 break 漏记）
    if ($ExitMode -eq "quit" -and $daemon_pid -ne 0) {
        $deadline = $t0.Add([TimeSpan]::FromSeconds(15))
        while ($daemonExitSec -lt 0 -and $sw.Elapsed -lt $deadline) {
            if (-not (Alive $daemon_pid)) {
                $daemonExitSec = [math]::Round(($sw.Elapsed - $t0).TotalSeconds, 1)
                break
            }
            Start-Sleep -Milliseconds 500
        }
        L ("daemon exited after ${daemonExitSec}s of quit (assert <=15s: {0})" -f ($daemonExitSec -ge 0 -and $daemonExitSec -le 15))
    }
    L "=== verdict: $verdict (elapsed $([math]::Round($sw.Elapsed.TotalSeconds))s) ==="
}
finally {
    # PID 精确杀（绝不 /IM）；僵尸（force 无效）不再重复杀。quit 臂先给
    # daemon ≤15s 自然退出窗口（桌面 1s 先退时别抢在优雅退出前强杀）。
    if ($daemon_pid -ne 0 -and $ExitMode -eq "quit" -and (Alive $daemon_pid)) {
        $deadline = [DateTime]::Now.AddSeconds(15)
        while ((Alive $daemon_pid) -and [DateTime]::Now -lt $deadline) { Start-Sleep -Milliseconds 500 }
    }
    foreach ($p in @($desktop_pid, $daemon_pid)) {
        if ($p -ne 0 -and (Alive $p)) {
            L "cleanup: Stop-Process pid=$p"
            Stop-Process -Id $p -Force -ErrorAction SilentlyContinue
        }
    }
    Start-Sleep -Seconds 2
    foreach ($p in @($desktop_pid, $daemon_pid)) {
        if ($p -ne 0) { L ("cleanup check pid={0} alive={1}" -f $p, (Alive $p)) }
    }
    L "=== repro finally done, verdict=$verdict ==="
}
