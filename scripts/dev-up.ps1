[CmdletBinding()]
param(
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$steps = @(
    New-FicantCheckStep -Name 'Build ficant-server' -FilePath 'cargo' -ArgumentList @('build', '--locked', '-p', 'ficant-server')
    New-FicantCheckStep -Name 'Run ficant-server on 127.0.0.1:50051' -FilePath 'cargo' -ArgumentList @('run', '--locked', '-p', 'ficant-server')
)

if ($ListOnly) {
    Show-FicantCheckPlan -Steps $steps
    exit 0
}

Assert-FicantRustCapability
Invoke-FicantCheckPlan -Steps $steps
