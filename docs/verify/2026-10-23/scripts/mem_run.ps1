# S6 §21 内存治理真机长跑编排（B 主实验）
# 隔离环境（env-s6.ps1）+ release hmp-desktop 真机 GUI + CLI 驱动 churn + 5s RSS 采样。
# 看门狗：总预算 28min（含启动序列），超时强杀自有 PID（Stop-Process -Id 精确杀）。
# 判据分析见 analyze_mem.py（本脚本只采样不判定）。
#
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File verify\scripts\mem_run.ps1

$ErrorActionPreference = "Continue"
. "$PSScriptRoot\env-s6.ps1"
if ([string]::IsNullOrEmpty($Global:S6Root) -or -not (Test-Path $Global:S6Hmp)) {
    throw "env-s6.ps1 load failed: S6Root/S6Hmp empty (encoding or path problem)"
}

$DataDir = Join-Path $Global:S6Root "data"
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
$Csv = Join-Path $DataDir "rss_curve.csv"
if (Test-Path $Csv) { Remove-Item $Csv }
[System.IO.File]::WriteAllText($Csv, "ts,elapsed_s,phase,pid,image,ws_kb,pm_kb`r`n")

$Log = Join-Path $DataDir "mem_run.log"
function L([string]$m) {
    $line = "[{0}] {1}" -f (Get-Date -Format "HH:mm:ss"), $m
    Write-Host $line
    [System.IO.File]::AppendAllText($Log, $line + "`r`n")
}
if (Test-Path $Log) { Remove-Item $Log }

$sw = [System.Diagnostics.Stopwatch]::StartNew()
$BudgetSec = 28 * 60          # 看门狗总预算（PLAN R-5）
$desktop_pid = 0
$daemon_pid = 0

# —— 采样：desktop + daemon 各记一行（daemon 为对照组）——
function Sample([string]$phase) {
    foreach ($spec in @(@($script:desktop_pid, "hmp-desktop"), @($script:daemon_pid, "hmp-daemon"))) {
        $p = $spec[0]; $img = $spec[1]
        if ($p -eq 0) { continue }
        $proc = Get-Process -Id $p -ErrorAction SilentlyContinue
        if ($null -eq $proc) { continue }
        $row = "{0},{1},{2},{3},{4},{5},{6}" -f (
            (Get-Date -Format "yyyy-MM-ddTHH:mm:ss"),
            [math]::Round($script:sw.Elapsed.TotalSeconds, 1),
            $phase, $p, $img,
            [math]::Round($proc.WorkingSet64 / 1KB, 0),
            [math]::Round($proc.PrivateMemorySize64 / 1KB, 0))
        [System.IO.File]::AppendAllText($script:Csv, $row + "`r`n")
    }
}

# —— 阶段执行器：phase <名, 秒, 动作脚本块（每轮 actionTick 秒跑一次）——
function Run-Phase([string]$name, [int]$secs, [double]$actionTick, [scriptblock]$action) {
    $end = $script:sw.Elapsed.Add([TimeSpan]::FromSeconds($secs))
    $nextAction = $script:sw.Elapsed
    $lastSample = [DateTime]::MinValue
    L "phase $name start (budget used $([math]::Round($script:sw.Elapsed.TotalSeconds))s)"
    while ($script:sw.Elapsed -lt $end) {
        if ($script:sw.Elapsed.TotalSeconds -gt $script:BudgetSec) { L "WATCHDOG TRIP in phase $name"; return }
        if ($null -ne $action -and $script:sw.Elapsed -ge $nextAction) {
            & $action | Out-Null
            $nextAction = $script:sw.Elapsed.Add([TimeSpan]::FromSeconds($actionTick))
        }
        if (((Get-Date) - $lastSample).TotalSeconds -ge 5 -or $lastSample -eq [DateTime]::MinValue) {
            Sample $name
            $lastSample = Get-Date
        }
        Start-Sleep -Milliseconds 250
    }
    L "phase $name end"
}

try {
    # ===== 启动序列（P0 前置，PLAN §3-B）=====
    L "=== S6 mem_run start ==="
    # 1) 旧隔离 daemon 清场（可能来自 T2 冒烟）：quit 只打隔离端点
    & $Global:S6Hmp quit 2>$null | Out-Null
    Start-Sleep -Seconds 3

    # 2) 起隔离 daemon
    $daemon = Start-Process -FilePath $Global:S6Hmp -ArgumentList "serve" -WindowStyle Hidden -PassThru
    $daemon_pid = $daemon.Id
    Start-Sleep -Seconds 4
    if (-not (Get-Process -Id $daemon_pid -ErrorAction SilentlyContinue)) { throw "daemon failed to start" }
    L "daemon pid=$daemon_pid"

    # 3) 声音门 + 指纹自检
    & $Global:S6Hmp volume 0 | Out-Null
    $statusJson = & $Global:S6Hmp status --json | ConvertFrom-Json
    if ($statusJson.playback.user_volume -ne 0) { throw "VOLUME GATE FAIL: user_volume=$($statusJson.playback.user_volume)" }
    L "volume gate PASS (user_volume=0)"
    $bb = $statusJson.backend_build
    if ([string]::IsNullOrEmpty("$bb")) { throw "backend_build empty (fingerprint self-check)" }
    L "backend_build=$bb"
    $exeH = Get-Item $Global:S6Hmp
    $exeD = Get-Item $Global:S6Desktop
    L ("exe same-batch check: hmp.exe mtime={0}, hmp-desktop.exe mtime={1} (same cargo invocation => 同批)" -f $exeH.LastWriteTime.ToString("HH:mm:ss"), $exeD.LastWriteTime.ToString("HH:mm:ss"))

    # 4) 起桌面（真机 GUI；约 25 分钟已获批）
    $desktop = Start-Process -FilePath $Global:S6Desktop -PassThru
    $desktop_pid = $desktop.Id
    L "desktop pid=$desktop_pid"
    Start-Sleep -Seconds 12
    if (-not (Get-Process -Id $desktop_pid -ErrorAction SilentlyContinue)) { throw "desktop failed to start" }

    # 素材路径表
    $media = Join-Path $Global:S6Root "sandbox\media"
    $tracks = Get-ChildItem $media -Filter "*.wav" | ForEach-Object { "local:" + $_.FullName }
    $nTracks = $tracks.Count
    L "media tracks: $nTracks"
    $rng = New-Object System.Random(20261023)
    $playPos = 0

    # ===== P0 warmup 3min：静置 =====
    Run-Phase "P0-warmup" 180 $null $null

    # ===== P1 播放/切曲 7min：起播 + 每 ~6s next（≈70 次）=====
    Run-Phase "P1-play" 420 6 {
        $st = & $Global:S6Hmp status --json 2>$null | ConvertFrom-Json
        if ($st.playback.status -ne "Playing") {
            & $Global:S6Hmp play "playlist:local:23" 2>$null | Out-Null
        } else {
            & $Global:S6Hmp next 2>$null | Out-Null
        }
    }

    # ===== P2 队列 churn 4min：add ×20 → clear 循环（确定性计数，不解析输出）=====
    $script:qCount = 0
    Run-Phase "P2-queue" 240 1.2 {
        if ($script:qCount -ge 20) {
            & $Global:S6Hmp queue clear 2>$null | Out-Null
            $script:qCount = 0
        } else {
            $t = $tracks[$rng.Next($nTracks)]
            & $Global:S6Hmp queue add $t 2>$null | Out-Null
            $script:qCount++
        }
    }

    # ===== P3 库 churn 4min：playlist add/remove（LibraryChanged → 全模型刷新）=====
    # 80 起步：加到 >90 后每 tick 先加一尾再删一头（净 +0/-1 交替，规模有界）
    $script:pCount = 80
    Run-Phase "P3-library" 240 2.5 {
        $t = $tracks[$rng.Next($nTracks)]
        & $Global:S6Hmp playlist add 23 $t 2>$null | Out-Null
        $script:pCount++
        if ($script:pCount -gt 90) {
            & $Global:S6Hmp playlist remove 23 0 2>$null | Out-Null
            $script:pCount--
        }
    }

    # ===== P4 settle 2min：无驱动 =====
    Run-Phase "P4-settle" 120 $null $null

    # ===== P5 复扫 3min：再次换曲 ~30 次 =====
    Run-Phase "P5-replay" 180 6 {
        $st = & $Global:S6Hmp status --json 2>$null | ConvertFrom-Json
        if ($st.playback.status -ne "Playing") {
            & $Global:S6Hmp play "playlist:local:23" 2>$null | Out-Null
        } else {
            & $Global:S6Hmp next 2>$null | Out-Null
        }
    }

    L "=== phases complete, elapsed $([math]::Round($sw.Elapsed.TotalSeconds))s ==="
}
finally {
    # ===== 收尾（T6 前置）：quit 隔离端点 + PID 精确杀 + 复核 =====
    L "cleanup: hmp quit (isolated endpoint)"
    & $Global:S6Hmp quit 2>$null | Out-Null
    Start-Sleep -Seconds 5
    foreach ($p in @($desktop_pid, $daemon_pid)) {
        if ($p -ne 0 -and (Get-Process -Id $p -ErrorAction SilentlyContinue)) {
            L "cleanup: Stop-Process pid=$p (still alive)"
            Stop-Process -Id $p -Force -ErrorAction SilentlyContinue
        }
    }
    Start-Sleep -Seconds 2
    foreach ($p in @($desktop_pid, $daemon_pid)) {
        $alive = [bool](Get-Process -Id $p -ErrorAction SilentlyContinue)
        L "cleanup check pid=$p alive=$alive"
    }
    L "=== mem_run done, total $([math]::Round($sw.Elapsed.TotalSeconds))s ==="
}
