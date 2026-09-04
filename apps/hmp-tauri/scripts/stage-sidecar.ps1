[CmdletBinding()]
param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$hostLine = rustc -vV | Select-String -Pattern '^host:\s+(.+)$'
if (-not $hostLine) {
    throw "Could not parse the host triple from rustc -vV. Check the Rust toolchain."
}
$targetTriple = $hostLine.Matches[0].Groups[1].Value.Trim()
$extension = if ($targetTriple -like "*windows*") { ".exe" } else { "" }
$source = Join-Path $repoRoot "target\$Profile\hmpd$extension"
$destinationDir = Join-Path $repoRoot "apps\hmp-tauri\src-tauri\binaries"
$destination = Join-Path $destinationDir "hmpd-$targetTriple$extension"

if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    $profileArg = if ($Profile -eq "release") { " --release" } else { "" }
    throw "hmpd was not found: $source`nRun: cargo build -p hmp-daemon --bin hmpd --no-default-features$profileArg"
}

New-Item -ItemType Directory -Path $destinationDir -Force | Out-Null
Copy-Item -LiteralPath $source -Destination $destination -Force
Write-Host "Staged Tauri sidecar: $destination"
