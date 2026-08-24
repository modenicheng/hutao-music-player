param(
    [Parameter(Mandatory = $true)]
    [string]$AppPath,

    [Parameter(Mandatory = $true)]
    [string]$Label,

    [ValidateRange(1, 30)]
    [int]$GpuSamples = 3,

    [ValidateRange(0, 30)]
    [int]$WarmupSeconds = 2,

    [ValidateRange(0, 60)]
    [int]$PreparationSeconds = 0
)

$ErrorActionPreference = 'Stop'
$resolvedApp = (Resolve-Path -LiteralPath $AppPath).Path
$token = [guid]::NewGuid().ToString('N')
$stdoutPath = Join-Path $env:TEMP "hmp-benchmark-$token.stdout.log"
$stderrPath = Join-Path $env:TEMP "hmp-benchmark-$token.stderr.log"
$process = $null
$result = $null

try {
    $startup = [Diagnostics.Stopwatch]::StartNew()
    $process = Start-Process `
        -FilePath $resolvedApp `
        -WorkingDirectory (Split-Path -Parent $resolvedApp) `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -PassThru

    $readyDeadline = (Get-Date).AddSeconds(15)
    do {
        Start-Sleep -Milliseconds 50
        $process.Refresh()
    } while (
        -not $process.HasExited -and
        $process.MainWindowHandle -eq 0 -and
        (Get-Date) -lt $readyDeadline
    )
    $startup.Stop()

    $ready = -not $process.HasExited -and $process.MainWindowHandle -ne 0
    if (-not $ready) {
        $result = [ordered]@{
            label = $Label
            pid = $process.Id
            ready = $false
            exited = $process.HasExited
            exit_code = if ($process.HasExited) { $process.ExitCode } else { $null }
            cold_start_ms = [math]::Round($startup.Elapsed.TotalMilliseconds, 1)
        }
    } else {
        if ($PreparationSeconds -gt 0) {
            Start-Sleep -Seconds $PreparationSeconds
        }
        if ($WarmupSeconds -gt 0) {
            Start-Sleep -Seconds $WarmupSeconds
        }

        $process.Refresh()
        $cpuStart = $process.CPU
        $sampleTimer = [Diagnostics.Stopwatch]::StartNew()
        $gpuAverage = $null
        try {
            $counter = Get-Counter `
                '\GPU Engine(*)\Utilization Percentage' `
                -SampleInterval 1 `
                -MaxSamples $GpuSamples
            $processGpu = $counter.CounterSamples |
                Where-Object { $_.InstanceName -like "pid_$($process.Id)*" }
            $gpuSums = $processGpu |
                Group-Object Timestamp |
                ForEach-Object { ($_.Group | Measure-Object CookedValue -Sum).Sum }
            if ($gpuSums) {
                $gpuAverage = ($gpuSums | Measure-Object -Average).Average
            }
        } catch {
            $gpuAverage = $null
        }
        $sampleTimer.Stop()

        $process.Refresh()
        $cpuDelta = $process.CPU - $cpuStart
        $logicalProcessors = [Environment]::ProcessorCount
        $result = [ordered]@{
            label = $Label
            pid = $process.Id
            ready = $true
            cold_start_ms = [math]::Round($startup.Elapsed.TotalMilliseconds, 1)
            working_set_mib = [math]::Round($process.WorkingSet64 / 1MB, 1)
            private_mib = [math]::Round($process.PrivateMemorySize64 / 1MB, 1)
            cpu_one_core_percent = [math]::Round(
                100 * $cpuDelta / $sampleTimer.Elapsed.TotalSeconds,
                2
            )
            cpu_system_percent = [math]::Round(
                100 * $cpuDelta / $sampleTimer.Elapsed.TotalSeconds / $logicalProcessors,
                3
            )
            gpu_engine_percent = if ($null -eq $gpuAverage) {
                $null
            } else {
                [math]::Round($gpuAverage, 2)
            }
            binary_mib = [math]::Round((Get-Item -LiteralPath $resolvedApp).Length / 1MB, 1)
        }
    }
} finally {
    if ($null -ne $process) {
        $process.Refresh()
        if (-not $process.HasExited) {
            [void]$process.CloseMainWindow()
            try {
                Wait-Process -Id $process.Id -Timeout 3 -ErrorAction Stop
            } catch {
                Stop-Process -Id $process.Id -ErrorAction SilentlyContinue
                Wait-Process -Id $process.Id -Timeout 3 -ErrorAction SilentlyContinue
            }
        }
    }

    $stderr = if (Test-Path -LiteralPath $stderrPath) {
        (Get-Content -LiteralPath $stderrPath -Tail 30 | Out-String).Trim()
    } else {
        ''
    }
    if ($null -ne $result) {
        $result['stderr'] = $stderr
        $result | ConvertTo-Json -Compress
    }

    Remove-Item -LiteralPath $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue
}
