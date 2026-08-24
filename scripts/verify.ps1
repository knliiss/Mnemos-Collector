Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "common.ps1")

Push-Location $script:RepositoryRoot

try {
    Write-Host "==> cargo fmt"
    Invoke-CargoCommand -Arguments @("fmt", "--all", "--", "--check")

    Write-Host "==> cargo clippy"
    Invoke-CargoCommand -Arguments @(
        "clippy",
        "--locked",
        "--all-targets",
        "--all-features",
        "--",
        "-D",
        "warnings"
    )

    Write-Host "==> cargo test"
    Invoke-CargoCommand -Arguments @(
        "test",
        "--locked",
        "--all-targets",
        "--all-features"
    )

    Write-Host ""
    Write-Host "Collector verification passed."
} finally {
    Pop-Location
}
