[CmdletBinding()]
param(
    [string]$InterfaceRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$descriptorInput = if ([string]::IsNullOrWhiteSpace($InterfaceRoot)) {
    Join-Path $repoRoot 'interface'
}
else {
    (Resolve-Path -LiteralPath $InterfaceRoot).Path
}
if (-not (Test-Path -LiteralPath $descriptorInput -PathType Container)) {
    throw "Coverage descriptor input is not a directory: $descriptorInput"
}

$bufCommand = if ([string]::IsNullOrWhiteSpace($env:FICANT_BUF)) {
    (Get-Command 'buf' -ErrorAction Stop).Source
}
else {
    (Get-Command $env:FICANT_BUF -ErrorAction Stop).Source
}
$bufVersion = (& $bufCommand --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $bufVersion -ne '1.56.0') {
    throw "Coverage gate requires fixed Buf 1.56.0, got '$bufVersion'."
}

$previousBuf = $env:FICANT_BUF
$previousDescriptorInput = $env:FICANT_DESCRIPTOR_INPUT
$env:FICANT_BUF = $bufCommand
$env:FICANT_DESCRIPTOR_INPUT = $descriptorInput
try {
    & cargo test --offline --locked -p ficant-contract-tests --test descriptor_inventory composition_level_outputs_have_coverage -- --exact
    if ($LASTEXITCODE -ne 0) {
        throw "Coverage descriptor inventory failed with exit code $LASTEXITCODE."
    }
}
finally {
    $env:FICANT_BUF = $previousBuf
    $env:FICANT_DESCRIPTOR_INPUT = $previousDescriptorInput
}

Write-Host 'Coverage descriptor gate passed: 3 reachable composition carriers require CoverageDeclaration; per-position payloads remain outside the inventory.'
exit 0
