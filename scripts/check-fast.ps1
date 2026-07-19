[CmdletBinding()]
param(
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$steps = @(
    New-FicantCheckStep -Name 'Rust formatting' -FilePath 'cargo' -ArgumentList @('fmt', '--all', '--', '--check')
    New-FicantCheckStep -Name 'Rust workspace check' -FilePath 'cargo' -ArgumentList @('check', '--offline', '--workspace', '--all-targets', '--locked')
    New-FicantCheckStep -Name 'Rust non-environment tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--workspace', '--locked', '--exclude', 'ficant-acceptance', '--exclude', 'ficant-data', '--exclude', 'ficant-storage', '--exclude', 'ficant-contract-tests')
    New-FicantCheckStep -Name 'Rust storage library tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--lib')
    New-FicantCheckStep -Name 'Phase 3A canonical data tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-data', '--test', 'canonical_ingestion')
    New-FicantCheckStep -Name 'Phase 3B deterministic snapshot codec' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-data', '--test', 'snapshot_codec')
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
