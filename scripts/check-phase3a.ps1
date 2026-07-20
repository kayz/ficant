[CmdletBinding()]
param(
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$steps = @(
    New-FicantCheckStep -Name 'Phase 3A data-source registry integration' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--test', 'data_source_registry_sit', '--', '--test-threads=1')
    New-FicantCheckStep -Name 'Phase 3A file/PostgreSQL canonical parity' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-data', '--test', 'dual_source_sit', '--', '--test-threads=1')
)

try {
    if ($ListOnly) {
        Show-FicantCheckPlan -Steps $steps
        exit 0
    }

    Assert-FicantRustCapability
    if ([string]::IsNullOrWhiteSpace($env:FICANT_TEST_DATABASE_URL)) {
        throw 'Phase 3A integration requires FICANT_TEST_DATABASE_URL to identify a disposable PostgreSQL 16 database.'
    }
    Invoke-FicantCheckPlan -Steps $steps
    Write-Host ''
    Write-Host 'FICANT Phase 3A integration checks passed.'
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
