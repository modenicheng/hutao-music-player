[CmdletBinding()]
param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$hostLine = rustc -vV | Select-String -Pattern '^host:\s+(.+)$'
if (-not $hostLine) {
    throw "无法从 rustc -vV 解析当前 host triple。请确认 Rust 工具链可用。"
}
$targetTriple = $hostLine.Matches[0].Groups[1].Value.Trim()
$extension = if ($targetTriple -like "*windows*") { ".exe" } else { "" }
$source = Join-Path $repoRoot "target\$Profile\hmpd$extension"
$destinationDir = Join-Path $repoRoot "apps\hmp-tauri\src-tauri\binaries"
$destination = Join-Path $destinationDir "hmpd-$targetTriple$extension"

if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    $profileArg = if ($Profile -eq "release") { " --release" } else { "" }
    throw "未找到 hmpd：$source`n请先运行：cargo build -p hmp-daemon --bin hmpd --no-default-features$profileArg"
}

New-Item -ItemType Directory -Path $destinationDir -Force | Out-Null
Copy-Item -LiteralPath $source -Destination $destination -Force
Write-Host "已暂存 Tauri sidecar：$destination"
