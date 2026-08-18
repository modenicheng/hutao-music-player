[CmdletBinding()]
param(
    [string]$Root
)

$ErrorActionPreference = "Stop"

$candidates = @(
    $Root,
    $env:GSTREAMER_1_0_ROOT_MSVC_X86_64,
    "C:\gstreamer\1.0\msvc_x86_64",
    (Join-Path ${env:ProgramFiles} "gstreamer\1.0\msvc_x86_64"),
    (Join-Path ${env:LOCALAPPDATA} "Programs\gstreamer\1.0\msvc_x86_64")
) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }

$gstreamerRoot = $candidates |
    Where-Object {
        (Test-Path -LiteralPath (Join-Path $_ "bin\gst-inspect-1.0.exe") -PathType Leaf) -and
        (Test-Path -LiteralPath (Join-Path $_ "lib\pkgconfig\gstreamer-1.0.pc") -PathType Leaf)
    } |
    Select-Object -First 1

if (-not $gstreamerRoot) {
    throw @"
GStreamer MSVC x86_64 SDK was not found.
Install matching Runtime and Development packages from https://gstreamer.freedesktop.org/download/,
or pass -Root with a directory containing bin\gst-inspect-1.0.exe and lib\pkgconfig\gstreamer-1.0.pc.
"@
}

$gstreamerRoot = (Resolve-Path -LiteralPath $gstreamerRoot).Path
$binPath = Join-Path $gstreamerRoot "bin"
$pkgConfigPath = Join-Path $gstreamerRoot "lib\pkgconfig"

$env:GSTREAMER_1_0_ROOT_MSVC_X86_64 = $gstreamerRoot
if (($env:PATH -split ';') -notcontains $binPath) {
    $env:PATH = "$binPath;$env:PATH"
}
$existingPkgConfig = $env:PKG_CONFIG_PATH
$env:PKG_CONFIG_PATH = if ([string]::IsNullOrWhiteSpace($existingPkgConfig)) {
    $pkgConfigPath
} elseif (($existingPkgConfig -split ';') -contains $pkgConfigPath) {
    $existingPkgConfig
} else {
    "$pkgConfigPath;$existingPkgConfig"
}

Write-Host "GStreamer SDK: $gstreamerRoot"
Write-Host "Configured PATH, PKG_CONFIG_PATH, and GSTREAMER_1_0_ROOT_MSVC_X86_64 for this process."
