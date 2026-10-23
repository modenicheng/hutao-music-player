# S9 E1 复验长跑编排（自 S6 mem_run.ps1 只读复制适配；看门狗 28min 原样继承）。
# 与 S6 原版差异（仅两处，见「E1-ADAPTED」标记）：
#   1) 臂环境经参数 dot-source（fix/base 两臂各自 env-s6.ps1，变量名沿用 $Global:S6*）
#   2) churn 歌单 id 动态发现（QQ 开机 reconcile 使本地歌单 id 漂移，S6 硬编码
#      的 23 不可复用）：setup 段创建「e1长跑」并播 80 曲后捕获真实 id，
#      P1/P3/P5 全部引用 $script:PlId。
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File verify\scripts\mem_run_e1.ps1 <armDir>
#   <armDir> = verify\arm-fix 或 verify\arm-base（内含 env-s6.ps1）

param([Parameter(Mandatory=$true)][string]$ArmDir)
$ErrorActionPreference = "Continue"
. (Join-Path (Resolve-Path $ArmDir) "env-s6.ps1")
if ([string]::IsNullOrEmpty($Global:S6Root) -or -not (Test-Path $Global:S6Hmp)) {
    throw "arm env-s6.ps1 load failed: S6Root/S6Hmp empty (encoding or path problem)"
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
$BudgetSec = 28 * 60          # 看门狗总预算（PLAN R-5，S6 原样）
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
    L "phase $name start (budget used $([math]::Round($sw.Elapsed.TotalSeconds))s)"
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
    # ===== 启动序列 =====
    L "=== S9 E1 mem_run start (arm=$ArmDir) ==="
    # 1) 旧隔离 daemon 清场（只打本臂隔离端点）
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

    # E1-ADAPTED 3.5) churn 歌单：扫描素材 + 创建本地歌单 + 播种 80 曲 + 捕获 id
    # （S6 靠开机 reconcile 后固定 id=23；账号歌单数已漂移，改为动态发现）
    $media = Join-Path $Global:S6Root "sandbox\media"
    & $Global:S6Hmp scan $media 2>$null | Out-Null
    L "scan done: $media"
    $created = & $Global:S6Hmp playlist create "e1长跑" 2>$null
    if ($created -notmatch '#(\d+)') { throw "playlist create failed: $created" }
    $script:PlId = [int]$Matches[1]
    L "churn playlist id=$script:PlId ($created)"
    $tracks = Get-ChildItem $media -Filter "*.wav" | ForEach-Object { "local:" + $_.FullName }
    $added = 0
    foreach ($t in $tracks) {
        & $Global:S6Hmp playlist add $script:PlId $t 2>$null | Out-Null
        $added++
    }
    L "seeded $added tracks into playlist $script:PlId"

    # 4) 起桌面（真机 GUI；~21min 已获批）
    $desktop = Start-Process -FilePath $Global:S6Desktop -PassThru
    $desktop_pid = $desktop.Id
    L "desktop pid=$desktop_pid"
    Start-Sleep -Seconds 12
    if (-not (Get-Process -Id $desktop_pid -ErrorAction SilentlyContinue)) { throw "desktop failed to start" }

    $nTracks = $tracks.Count
    L "media tracks: $nTracks"
    $rng = New-Object System.Random(20261023)

    # ===== P0 warmup 3min：静置 =====
    Run-Phase "P0-warmup" 180 $null $null

    # ===== P1 播放/切曲 7min：起播 + 每 ~6s next（≈70 次）=====
    Run-Phase "P1-play" 420 6 {
        $st = & $Global:S6Hmp status --json 2>$null | ConvertFrom-Json
        if ($st.playback.status -ne "Playing") {
            & $Global:S6Hmp play "playlist:local:$script:PlId" 2>$null | Out-Null
        } else {
            & $Global:S6Hmp next 2>$null | Out-Null
        }
    }

    # ===== P2 队列 churn 4min：add ×20 → clear 循环 =====
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
    $script:pCount = 80
    Run-Phase "P3-library" 240 2.5 {
        $t = $tracks[$rng.Next($nTracks)]
        & $Global:S6Hmp playlist add $script:PlId $t 2>$null | Out-Null
        $script:pCount++
        if ($script:pCount -gt 90) {
            & $Global:S6Hmp playlist remove $script:PlId 0 2>$null | Out-Null
            $script:pCount--
        }
    }

    # ===== P4 settle 2min：无驱动 =====
    Run-Phase "P4-settle" 120 $null $null

    # ===== P5 复扫 3min：再次换曲 ~30 次 =====
    Run-Phase "P5-replay" 180 6 {
        $st = & $Global:S6Hmp status --json 2>$null | ConvertFrom-Json
        if ($st.playback.status -ne "Playing") {
            & $Global:S6Hmp play "playlist:local:$script:PlId" 2>$null | Out-Null
        } else {
            & $Global:S6Hmp next 2>$null | Out-Null
        }
    }

    L "=== phases complete, elapsed $([math]::Round($sw.Elapsed.TotalSeconds))s ==="
}
finally {
    # ===== 收尾：quit 隔离端点 + PID 精确杀 + 复核 =====
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
