param(
    [Parameter(Position = 0)]
    [ValidateSet("verify", "build", "run", "stop", "status", "logs", "smoke", "reset-onboarding")]
    [string]$Command = "status",

    [switch]$ProductionLike,
    [switch]$SkipBuild,
    [switch]$ConfirmReset,
    [switch]$Follow
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptsDirectory = Join-Path $PSScriptRoot "scripts"
$verifyScript = Join-Path $scriptsDirectory "verify.ps1"
$buildScript = Join-Path $scriptsDirectory "build.ps1"
$localScript = Join-Path $scriptsDirectory "local.ps1"

switch ($Command) {
    "verify" {
        & $verifyScript
    }
    "build" {
        & $buildScript -ProductionLike:$ProductionLike
    }
    default {
        & $localScript `
            -Action $Command `
            -ProductionLike:$ProductionLike `
            -SkipBuild:$SkipBuild `
            -ConfirmReset:$ConfirmReset `
            -Follow:$Follow
    }
}
