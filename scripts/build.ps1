param(
    [switch]$ProductionLike
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "common.ps1")

$previousUpdatePublicKey = [Environment]::GetEnvironmentVariable(
    "MNEMOS_COLLECTOR_UPDATE_PUBLIC_KEY",
    "Process"
)

Push-Location $script:RepositoryRoot

try {
    if ($ProductionLike) {
        $env:MNEMOS_COLLECTOR_UPDATE_PUBLIC_KEY = Resolve-ProductionUpdatePublicKey
        Write-Host "==> Production-like update verification enabled"
    }

    Write-Host "==> cargo build --release"
    Invoke-CargoCommand -Arguments @("build", "--locked", "--release")

    $binaryName = "mnemos-collector"

    if ($script:IsWindowsHost) {
        $binaryName = "$binaryName.exe"
    }

    $releaseDirectory = Join-Path (Join-Path $script:RepositoryRoot "target") "release"
    $binaryPath = Join-Path $releaseDirectory $binaryName

    if (-not (Test-Path $binaryPath)) {
        throw "Release binary was not produced at $binaryPath"
    }

    Write-Host ""
    Write-Host "Built: $binaryPath"
} finally {
    if ($null -eq $previousUpdatePublicKey) {
        Remove-Item Env:MNEMOS_COLLECTOR_UPDATE_PUBLIC_KEY -ErrorAction SilentlyContinue
    } else {
        $env:MNEMOS_COLLECTOR_UPDATE_PUBLIC_KEY = $previousUpdatePublicKey
    }

    Pop-Location
}
