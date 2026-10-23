# S6 B 浏览代理跑：shot-menu --browse-loop（窗口化直驱 + 判别法早期门）
# 独立采样 RSS 到 rss_browse_proxy.csv；看门狗 10min；PID 精确杀。
# 运行方必须先隔离（本脚本 dot-source env-s6.ps1）。
$ErrorActionPreference = "Continue"
. "$PSScriptRoot\env-s6.ps1"

$DataDir = Join-Path $Global:S6Root "data"
$Csv = Join-Path $DataDir "rss_browse_proxy.csv"
$Blog = Join-Path $DataDir "browse_proxy.log"
if (Test-Path $Csv) { Remove-Item $Csv }
[System.IO.File]::WriteAllText($Csv, "ts,elapsed_s,phase,pid,image,ws_kb,pm_kb`r`n")

$sw = [System.Diagnostics.Stopwatch]::StartNew()
$proc = Start-Process -FilePath $Global:S6ShotMenu -ArgumentList @("library", "--browse-loop", "--browse-minutes", "6", "--rows", "4500", "--browse-force") -WindowStyle Hidden -PassThru -RedirectStandardOutput $Blog
$proxy_pid = $proc.Id
Write-Host "browse proxy pid=$proxy_pid"

try {
    while (-not $proc.HasExited) {
        if ($sw.Elapsed.TotalSeconds -gt 600) { Write-Host "WATCHDOG TRIP"; break }
        $p = Get-Process -Id $proxy_pid -ErrorAction SilentlyContinue
        if ($null -eq $p) { break }
        $row = "{0},{1},{2},{3},{4},{5},{6}" -f (
            (Get-Date -Format "yyyy-MM-ddTHH:mm:ss"),
            [math]::Round($sw.Elapsed.TotalSeconds, 1),
            "browse-proxy", $proxy_pid, "shot-menu",
            [math]::Round($p.WorkingSet64 / 1KB, 0),
            [math]::Round($p.PrivateMemorySize64 / 1KB, 0))
        [System.IO.File]::AppendAllText($Csv, $row + "`r`n")
        Start-Sleep -Seconds 5
    }
}
finally {
    if (-not $proc.HasExited) {
        Stop-Process -Id $proxy_pid -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 2
    }
    Write-Host ("alive after cleanup: " + [bool](Get-Process -Id $proxy_pid -ErrorAction SilentlyContinue))
    # shot-menu 进程内 connect_or_spawn 可能拉起隔离 daemon——对隔离端点收尾
    & $Global:S6Hmp quit 2>$null | Out-Null
    Start-Sleep -Seconds 2
    Write-Host "browse proxy run done, total $([math]::Round($sw.Elapsed.TotalSeconds))s"
}
