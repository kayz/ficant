[CmdletBinding()]
param(
    [switch]$Check
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$packs = @(
    @{
        Name = 'v1'
        Source = Join-Path $repoRoot 'domain-packs\cgb-futures\cgb-futures-v1.json'
        Target = Join-Path $repoRoot 'domain-packs\cgb-futures\cgb-futures-v1.bin'
    },
    @{
        Name = 'v2'
        Source = Join-Path $repoRoot 'domain-packs\cgb-futures\cgb-futures-v2.json'
        Target = Join-Path $repoRoot 'domain-packs\cgb-futures\cgb-futures-v2.bin'
    }
)
$bufExecutable = if ([string]::IsNullOrWhiteSpace($env:FICANT_BUF)) {
    (Get-Command buf -ErrorAction Stop).Source
}
else {
    $env:FICANT_BUF
}

if (-not (Test-Path -LiteralPath $bufExecutable -PathType Leaf)) {
    throw "Buf executable is missing: $bufExecutable"
}
$bufVersion = (& $bufExecutable --version).Trim()
if ($LASTEXITCODE -ne 0 -or $bufVersion -ne '1.56.0') {
    throw 'CGB futures RulePack generation requires Buf 1.56.0; set FICANT_BUF to a verified executable.'
}

$temporaryPayloads = [System.Collections.Generic.List[string]]::new()
try {
    foreach ($pack in $packs) {
        $source = $pack.Source
        $target = $pack.Target
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
            throw "CGB futures RulePack JSON is missing: $source"
        }
        $temporaryPayload = Join-Path ([System.IO.Path]::GetTempPath()) (
            'ficant-cgb-futures-' + $pack.Name + '-' + [guid]::NewGuid().ToString('N') + '.bin'
        )
        $temporaryPayloads.Add($temporaryPayload)
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
                throw "cgb-futures RulePack $($pack.Name) binary is missing or stale; rerun generate-cgb-futures-pack.ps1"
            }
        }
        else {
            [System.IO.File]::WriteAllBytes($target, $payload)
        }
    }
}
finally {
    foreach ($temporaryPayload in $temporaryPayloads) {
        if (Test-Path -LiteralPath $temporaryPayload -PathType Leaf) {
            Remove-Item -LiteralPath $temporaryPayload -Force
        }
    }
}
