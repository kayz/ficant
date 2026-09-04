[CmdletBinding()]
param(
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$steps = @(
    New-FicantCheckStep -Name 'Coverage descriptor gate' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/check-coverage.ps1')
    New-FicantCheckStep -Name 'Coverage gate fixture tests' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/test-coverage-check.ps1')
    New-FicantCheckStep -Name 'R7B MANUAL literal-runner fixture tests' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/test-manual-check.ps1')
    New-FicantCheckStep -Name 'R7B recovery-manifest fixture tests' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/test-recovery-check.ps1')
    New-FicantCheckStep -Name 'Rust formatting' -FilePath 'cargo' -ArgumentList @('fmt', '--all', '--', '--check')
    New-FicantCheckStep -Name 'Rust workspace check' -FilePath 'cargo' -ArgumentList @('check', '--offline', '--workspace', '--all-targets', '--locked')
    New-FicantCheckStep -Name 'R5D layer dependency gate' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-contract-tests', '--test', 'r5d_layer_dependencies')
    New-FicantCheckStep -Name 'R7A zero-core-change extension gate' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-contract-tests', '--test', 'r7a_core_extension')
    New-FicantCheckStep -Name 'R7B formal evidence contract gate' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-contract-tests', '--test', 'r7b_formal_evidence')
    New-FicantCheckStep -Name 'R8A Portfolio contract gate' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-contract-tests', '--test', 'r8a_portfolio_contract')
    New-FicantCheckStep -Name 'R8B Portfolio Performance contract gate' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-contract-tests', '--test', 'r8b_portfolio_performance_contract')
    New-FicantCheckStep -Name 'R8B deterministic local contract package' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/test-contract-package.ps1', '-RemoveRepositoryOutput')
    New-FicantCheckStep -Name 'R8B descriptor-to-production topology gate' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-server', '--test', 'service_topology')
    New-FicantCheckStep -Name 'R8B Portfolio Performance domain arithmetic' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-domain', '--test', 'r8b_portfolio_performance')
    New-FicantCheckStep -Name 'R8B Portfolio Performance exact materialization' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-application', '--test', 'r8b_portfolio_performance')
    New-FicantCheckStep -Name 'Rust non-environment tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--workspace', '--locked', '--exclude', 'ficant-acceptance', '--exclude', 'ficant-data', '--exclude', 'ficant-storage', '--exclude', 'ficant-contract-tests')
    New-FicantCheckStep -Name 'Rust storage library tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-storage', '--lib')
    New-FicantCheckStep -Name 'Phase 3A canonical data tests' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-data', '--test', 'canonical_ingestion')
    New-FicantCheckStep -Name 'Phase 3B deterministic snapshot codec' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-data', '--test', 'snapshot_codec')
    New-FicantCheckStep -Name 'R8A Portfolio Catalog API' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-api', '--test', 'portfolio_catalog_service')
    New-FicantCheckStep -Name 'R8A Portfolio Aggregation API' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-api', '--test', 'portfolio_aggregation_service')
    New-FicantCheckStep -Name 'R8A Portfolio Workbench API' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-api', '--test', 'portfolio_workbench_service')
    New-FicantCheckStep -Name 'R8B Portfolio Performance API' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-api', '--test', 'portfolio_performance_service')
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
