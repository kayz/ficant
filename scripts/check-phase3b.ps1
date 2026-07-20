[CmdletBinding()]
param(
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$steps = @(
    New-FicantCheckStep -Name 'Phase 3B deterministic snapshot codec' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-data', '--test', 'snapshot_codec')
    New-FicantCheckStep -Name 'Phase 3B immutable snapshot publication' -FilePath 'cargo' -ArgumentList @('test', '--offline', '--locked', '-p', 'ficant-data', '--test', 'snapshot_publication_sit', '--', '--test-threads=1')
)

try {
    if ($ListOnly) {
        Show-FicantCheckPlan -Steps $steps
        exit 0
    }

    Assert-FicantRustCapability
    $requiredVariables = @(
        'FICANT_TEST_DATABASE_URL',
        'FICANT_TEST_S3_ENDPOINT',
        'FICANT_TEST_S3_BUCKET',
        'FICANT_TEST_S3_ACCESS_KEY',
        'FICANT_TEST_S3_SECRET_KEY'
    )
    $missingVariables = @($requiredVariables | Where-Object {
        [string]::IsNullOrWhiteSpace([System.Environment]::GetEnvironmentVariable($_))
    })
    if ($missingVariables.Count -gt 0) {
        throw "Phase 3B integration requires disposable PostgreSQL 16 and Ceph RGW endpoints. Missing environment variables: $($missingVariables -join ', ')"
    }

    Invoke-FicantCheckPlan -Steps $steps
    Write-Host ''
    Write-Host 'FICANT Phase 3B integration checks passed.'
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
