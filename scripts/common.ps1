Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$script:RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$script:IsWindowsHost = [System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT

$script:CollectorDataRoot = $null
$script:CollectorInstallDirectory = $null
$script:CollectorInstallPath = $null
$script:CollectorLogPath = $null
$script:CollectorReleasePath = $null
$script:CollectorAutostartRegistryPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$script:CollectorAutostartValueName = "Mnemos Collector"
$script:CollectorCredentialTargets = @(
    "collector-access-key.mnemos-collector",
    "pending-provisioning.mnemos-collector"
)

if ($script:IsWindowsHost) {
    $script:CollectorDataRoot = Join-Path $env:LOCALAPPDATA "knalis\Mnemos Collector\data"
    $script:CollectorInstallDirectory = Join-Path $script:CollectorDataRoot "bin"
    $script:CollectorInstallPath = Join-Path $script:CollectorInstallDirectory "mnemos-collector.exe"
    $script:CollectorLogPath = Join-Path $script:CollectorDataRoot "logs\collector.log"

    $releaseDirectory = Join-Path (Join-Path $script:RepositoryRoot "target") "release"
    $script:CollectorReleasePath = Join-Path $releaseDirectory "mnemos-collector.exe"
}

function Assert-WindowsHost {
    if (-not $script:IsWindowsHost) {
        throw "This local Collector command is Windows-only."
    }
}

function Invoke-CargoCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    & cargo @Arguments

    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Stop-CollectorProcess {
    $processes = @(Get-Process -Name "mnemos-collector" -ErrorAction SilentlyContinue)

    if ($processes.Count -eq 0) {
        return
    }

    foreach ($process in $processes) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }

    $deadline = [DateTime]::UtcNow.AddSeconds(5)

    do {
        Start-Sleep -Milliseconds 100
        $remaining = @(Get-Process -Name "mnemos-collector" -ErrorAction SilentlyContinue)
    } while ($remaining.Count -gt 0 -and [DateTime]::UtcNow -lt $deadline)

    if ($remaining.Count -gt 0) {
        throw "Collector did not stop within 5 seconds."
    }
}

function Get-CollectorProcesses {
    Get-Process -Name "mnemos-collector" -ErrorAction SilentlyContinue
}

function Test-CredentialTarget {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Target
    )

    $output = & cmdkey.exe /list 2>$null

    return $null -ne ($output | Select-String -SimpleMatch $Target | Select-Object -First 1)
}

function Remove-CredentialTarget {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Target
    )

    & cmdkey.exe "/delete:$Target" *> $null
}

function Get-AutostartCommand {
    try {
        $properties = Get-ItemProperty `
            -Path $script:CollectorAutostartRegistryPath `
            -Name $script:CollectorAutostartValueName `
            -ErrorAction Stop

        return $properties.PSObject.Properties[$script:CollectorAutostartValueName].Value
    } catch {
        return $null
    }
}

function Resolve-ProductionUpdatePublicKey {
    if (-not [string]::IsNullOrWhiteSpace($env:MNEMOS_COLLECTOR_UPDATE_PUBLIC_KEY)) {
        return $env:MNEMOS_COLLECTOR_UPDATE_PUBLIC_KEY.Trim()
    }

    $gh = Get-Command gh -ErrorAction SilentlyContinue

    if ($null -eq $gh) {
        throw "Production-like build requires MNEMOS_COLLECTOR_UPDATE_PUBLIC_KEY or an authenticated gh CLI."
    }

    $output = (& $gh.Source variable get MNEMOS_COLLECTOR_UPDATE_PUBLIC_KEY -R knliiss/Mnemos-Collector 2>&1 | Out-String).Trim()

    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($output)) {
        throw "Failed to read MNEMOS_COLLECTOR_UPDATE_PUBLIC_KEY from GitHub Actions variables."
    }

    return $output
}

function Show-CollectorStatus {
    Assert-WindowsHost

    $processes = @(Get-CollectorProcesses)
    $autostart = Get-AutostartCommand

    Write-Host "Collector local status"
    Write-Host "  Processes:      $($processes.Count)"
    Write-Host "  Installed exe:  $(Test-Path $script:CollectorInstallPath)"
    Write-Host "  Install path:   $script:CollectorInstallPath"
    Write-Host "  Autostart:      $(-not [string]::IsNullOrWhiteSpace($autostart))"
    Write-Host "  Log path:       $script:CollectorLogPath"

    foreach ($target in $script:CollectorCredentialTargets) {
        Write-Host "  Credential $target`: $(Test-CredentialTarget $target)"
    }

    if ($processes.Count -gt 0) {
        Write-Host ""
        Write-Host "Running process IDs: $($processes.Id -join ', ')"
    }
}
