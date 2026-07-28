[CmdletBinding()]
param(
    [switch]$Check
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$source = Join-Path $repoRoot 'domain-packs\cgb-futures\cgb-futures-v1.json'
$target = Join-Path $repoRoot 'domain-packs\cgb-futures\cgb-futures-v1.bin'
$bufExecutable = if ([string]::IsNullOrWhiteSpace($env:FICANT_BUF)) {
    (Get-Command buf -ErrorAction Stop).Source
}
else {
    $env:FICANT_BUF
}

if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    throw "CGB futures RulePack JSON is missing: $source"
}
if (-not (Test-Path -LiteralPath $bufExecutable -PathType Leaf)) {
    throw "Buf executable is missing: $bufExecutable"
}
$bufVersion = (& $bufExecutable --version).Trim()
if ($LASTEXITCODE -ne 0 -or $bufVersion -ne '1.56.0') {
    throw 'CGB futures RulePack generation requires Buf 1.56.0; set FICANT_BUF to a verified executable.'
}

$temporaryPayload = Join-Path ([System.IO.Path]::GetTempPath()) (
    'ficant-cgb-futures-' + [guid]::NewGuid().ToString('N') + '.bin'
)
try {
    & $bufExecutable convert interface --type ficant.market.v1.CgbFuturesDeliveryRulePack `
        --from "$source#format=json" --to "$temporaryPayload#format=binpb"
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
    $payload = [System.IO.File]::ReadAllBytes($temporaryPayload)
    if ($Check) {
        $matches = Test-Path -LiteralPath $target -PathType Leaf
        if ($matches) {
            $existing = [System.IO.File]::ReadAllBytes($target)
            $matches = $existing.Length -eq $payload.Length
            for ($index = 0; $matches -and $index -lt $payload.Length; $index++) {
                if ($existing[$index] -ne $payload[$index]) {
                    $matches = $false
                }
            }
        }
        if (-not $matches) {
            throw 'cgb-futures RulePack binary is missing or stale; rerun generate-cgb-futures-pack.ps1'
        }
    }
    else {
        [System.IO.File]::WriteAllBytes($target, $payload)
    }
}
finally {
    if (Test-Path -LiteralPath $temporaryPayload -PathType Leaf) {
        Remove-Item -LiteralPath $temporaryPayload -Force
    }
}
