param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("run", "stop", "status", "logs", "smoke", "reset-onboarding")]
    [string]$Action,

    [switch]$ProductionLike,
    [switch]$SkipBuild,
    [switch]$ConfirmReset,
    [switch]$Follow
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "common.ps1")

Assert-WindowsHost

function Ensure-ReleaseBuild {
    if ($SkipBuild) {
        if (-not (Test-Path $script:CollectorReleasePath)) {
            throw "-SkipBuild was used, but $script:CollectorReleasePath does not exist."
        }

        return
    }

    & (Join-Path $PSScriptRoot "build.ps1") -ProductionLike:$ProductionLike
}

function Copy-ReleaseIntoStableInstallation {
    if (-not (Test-Path $script:CollectorReleasePath)) {
        throw "Release binary does not exist at $script:CollectorReleasePath"
    }

    New-Item -ItemType Directory -Path $script:CollectorInstallDirectory -Force | Out-Null

    Copy-Item `
        -LiteralPath $script:CollectorReleasePath `
        -Destination $script:CollectorInstallPath `
        -Force
}

function Start-StableCollector {
    if (-not (Test-Path $script:CollectorInstallPath)) {
        throw "Collector is not installed at $script:CollectorInstallPath"
    }

    Start-Process -FilePath $script:CollectorInstallPath
}

function Start-LocalReleaseCollector {
    if (-not (Test-Path $script:CollectorReleasePath)) {
        throw "Release binary does not exist at $script:CollectorReleasePath"
    }

    Start-Process -FilePath $script:CollectorReleasePath
}

function Get-LogLineCount {
    if (-not (Test-Path $script:CollectorLogPath)) {
        return 0
    }

    return (Get-Content -LiteralPath $script:CollectorLogPath | Measure-Object -Line).Lines
}

function Show-NewSmokeLog {
    param(
        [Parameter(Mandatory = $true)]
        [int]$BaselineLineCount
    )

    if (-not (Test-Path $script:CollectorLogPath)) {
        Write-Host "No Collector log was created during the smoke window."
        return
    }

    $newLines = @(
        Get-Content -LiteralPath $script:CollectorLogPath |
            Select-Object -Skip $BaselineLineCount
    )

    if ($newLines.Count -eq 0) {
        Write-Host "No new Collector log lines were written during the smoke window."
        return
    }

    $rateLimited = @($newLines | Select-String -SimpleMatch "429 Too Many Requests")
    $authenticated = @($newLines | Select-String -SimpleMatch "Authenticated WebSocket connected")
    $observing = @($newLines | Select-String -SimpleMatch "acknowledged OBSERVING")

    Write-Host "Realtime smoke evidence"
    Write-Host "  Authenticated connections: $($authenticated.Count)"
    Write-Host "  OBSERVING acknowledgements: $($observing.Count)"
    Write-Host "  HTTP 429 responses:         $($rateLimited.Count)"

    if ($rateLimited.Count -gt 0) {
        throw "Realtime smoke detected HTTP 429 rate limiting."
    }

    if ($authenticated.Count -gt 1) {
        throw "Realtime smoke detected repeated authenticated reconnects during the stability window."
    }

    if ($authenticated.Count -eq 0) {
        Write-Warning "Realtime authentication was not exercised. Check backend availability and current Collector credentials."
    }
}

switch ($Action) {
    "run" {
        Ensure-ReleaseBuild
        Stop-CollectorProcess
        Copy-ReleaseIntoStableInstallation
        Start-StableCollector
        Start-Sleep -Seconds 1

        Show-CollectorStatus
    }
    "stop" {
        Stop-CollectorProcess
        Write-Host "Collector stopped."
    }
    "status" {
        Show-CollectorStatus
    }
    "logs" {
        if (-not (Test-Path $script:CollectorLogPath)) {
            throw "Collector log does not exist at $script:CollectorLogPath"
        }

        if ($Follow) {
            Get-Content -LiteralPath $script:CollectorLogPath -Tail 100 -Wait
        } else {
            Get-Content -LiteralPath $script:CollectorLogPath -Tail 100
        }
    }
    "smoke" {
        Ensure-ReleaseBuild
        Stop-CollectorProcess
        Copy-ReleaseIntoStableInstallation

        $baselineLineCount = Get-LogLineCount

        Start-StableCollector
        Start-Sleep -Seconds 2

        $firstProcesses = @(Get-CollectorProcesses)

        if ($firstProcesses.Count -ne 1) {
            throw "Expected exactly one Collector process after startup, found $($firstProcesses.Count)."
        }

        Start-StableCollector
        Start-Sleep -Seconds 2

        $secondProcesses = @(Get-CollectorProcesses)

        if ($secondProcesses.Count -ne 1) {
            throw "Single-instance smoke failed: expected one Collector process, found $($secondProcesses.Count)."
        }

        Write-Host "Single-instance smoke passed. Waiting 35 seconds for realtime stability..."
        Start-Sleep -Seconds 35

        $stableProcesses = @(Get-CollectorProcesses)

        if ($stableProcesses.Count -ne 1) {
            throw "Collector process count changed during the stability window: $($stableProcesses.Count)."
        }

        Show-NewSmokeLog -BaselineLineCount $baselineLineCount

        Write-Host ""
        Write-Host "Local Collector smoke passed."
    }
    "reset-onboarding" {
        if (-not $ConfirmReset) {
            throw "reset-onboarding deletes the local Collector installation, spool, logs, autostart entry, and credentials. Re-run with -ConfirmReset."
        }

        Ensure-ReleaseBuild
        Stop-CollectorProcess

        Remove-ItemProperty `
            -Path $script:CollectorAutostartRegistryPath `
            -Name $script:CollectorAutostartValueName `
            -ErrorAction SilentlyContinue

        foreach ($target in $script:CollectorCredentialTargets) {
            Remove-CredentialTarget -Target $target
        }

        Remove-Item `
            -LiteralPath $script:CollectorDataRoot `
            -Recurse `
            -Force `
            -ErrorAction SilentlyContinue

        Start-LocalReleaseCollector

        Write-Host "Local Collector state was reset. The clean activation UI has been launched."
        Write-Warning "The previous Collector credential cannot be restored; completing activation requires a new activation token."
    }
}
