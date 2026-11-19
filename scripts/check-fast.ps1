[CmdletBinding()]
param(
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$steps = @(
    New-FicantCheckStep -Name 'Layering gate' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/check-layering.ps1')
    New-FicantCheckStep -Name 'Buf lint' -FilePath 'buf' -ArgumentList @('lint', 'interface')
    New-FicantCheckStep -Name 'Rust formatting' -FilePath 'cargo' -ArgumentList @('fmt', '--all', '--', '--check')
    New-FicantCheckStep -Name 'Rust workspace check' -FilePath 'cargo' -ArgumentList @('check', '--workspace', '--all-targets', '--locked')
    New-FicantCheckStep -Name 'Rust workspace tests' -FilePath 'cargo' -ArgumentList @('test', '--workspace', '--locked')
    New-FicantCheckStep -Name 'Python contracts and live SDK' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', '-q', 'python/tests')
)

try {
    if ($ListOnly) {
        Show-FicantCheckPlan -Steps $steps
        exit 0
    }

    Assert-FicantRustCapability
    Invoke-FicantCheckPlan -Steps $steps
    Write-Host ''
    Write-Host 'FICANT fast local checks passed.'
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
