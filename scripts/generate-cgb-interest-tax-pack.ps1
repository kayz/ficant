[CmdletBinding()]
param(
    [switch]$Check
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$packDirectory = Join-Path $repoRoot 'domain-packs\cgb-interest-tax'
$source = Join-Path $packDirectory 'cgb-interest-tax-v1.json'
$target = Join-Path $packDirectory 'cgb-interest-tax-v1.bin'
$manifest = Join-Path $packDirectory 'cgb-interest-tax-source-manifest.json'
$expectedSemanticHash = '54fa5adbeb8b164dc779ecc250ab622ab5747cdeb36f2b6da58f4d877ce5106a'
$expectedPayloadHash = '14748fb4d27d01b35ebe466f72669937c850fd48f9bbd875542848d3800168db'
$expectedSourceHash = '5108568626960a4d82448a250c1c7fa00368dc382a975bd4b0eea870cfe8f54f'
$expectedManifestHash = '211ec12aadd0f5072cd9c7b40fb439f1beb0ab0a9f3f1004ae3ee68738fe7d8c'
$bufExecutable = if ([string]::IsNullOrWhiteSpace($env:FICANT_BUF)) {
    (Get-Command buf -ErrorAction Stop).Source
}
else {
    $env:FICANT_BUF
}

if (-not (Test-Path -LiteralPath $bufExecutable -PathType Leaf)) {
    throw "Buf executable is missing: $bufExecutable"
}
if ((& $bufExecutable --version).Trim() -ne '1.56.0' -or $LASTEXITCODE -ne 0) {
    throw 'CGB interest-tax RulePack generation requires Buf 1.56.0.'
}
foreach ($required in @($source, $manifest)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "CGB interest-tax input is missing: $required"
    }
}

function Get-CanonicalUtf8Bytes {
    param([Parameter(Mandatory)][string]$Path)

    $utf8 = [System.Text.UTF8Encoding]::new($false, $true)
    try {
        $text = $utf8.GetString([System.IO.File]::ReadAllBytes($Path))
    }
    catch {
        throw "Authority text is not strict UTF-8: $Path"
    }
    if ($text.Length -gt 0 -and $text[0] -eq [char]0xFEFF) {
        throw "Authority text must not contain a UTF-8 BOM: $Path"
    }
    $withoutCrLf = $text.Replace("`r`n", '')
    if ($withoutCrLf.Contains("`r")) {
        throw "Authority text contains a lone carriage return: $Path"
    }
    $utf8.GetBytes($text.Replace("`r`n", "`n"))
}

$sourceHash = [Convert]::ToHexString(
    [System.Security.Cryptography.SHA256]::HashData((Get-CanonicalUtf8Bytes -Path $source))
).ToLowerInvariant()
$manifestHash = [Convert]::ToHexString(
    [System.Security.Cryptography.SHA256]::HashData((Get-CanonicalUtf8Bytes -Path $manifest))
).ToLowerInvariant()
if ($sourceHash -ne $expectedSourceHash) {
    throw "Authority canonical JSON drifted: expected $expectedSourceHash, got $sourceHash"
}
if ($manifestHash -ne $expectedManifestHash) {
    throw "Authority source manifest drifted: expected $expectedManifestHash, got $manifestHash"
}

$manifestValue = Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json -Depth 100
$normalizedJson = $manifestValue.normalized_facts | ConvertTo-Json -Depth 100 -Compress
$normalizedBytes = [System.Text.UTF8Encoding]::new($false).GetBytes($normalizedJson)
$semanticHash = [Convert]::ToHexString(
    [System.Security.Cryptography.SHA256]::HashData($normalizedBytes)
).ToLowerInvariant()
if ($semanticHash -ne $expectedSemanticHash -or
    $manifestValue.normalized_facts_sha256 -ne $expectedSemanticHash) {
    throw "Authority normalized facts drifted: expected $expectedSemanticHash, got $semanticHash"
}

$temporaryPayload = Join-Path ([System.IO.Path]::GetTempPath()) (
    'ficant-cgb-interest-tax-' + [guid]::NewGuid().ToString('N') + '.bin'
)
try {
    & $bufExecutable convert interface --type ficant.market.v1.TaxRulePackV2 `
        --from "$source#format=json" --to "$temporaryPayload#format=binpb"
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
    $payload = [System.IO.File]::ReadAllBytes($temporaryPayload)
    $payloadHash = [Convert]::ToHexString(
        [System.Security.Cryptography.SHA256]::HashData($payload)
    ).ToLowerInvariant()
    if ($payloadHash -ne $expectedPayloadHash) {
        throw "Authority protobuf payload drifted: expected $expectedPayloadHash, got $payloadHash"
    }
    if ($Check) {
        if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {
            throw 'CGB interest-tax RulePack binary is missing.'
        }
        $existing = [System.IO.File]::ReadAllBytes($target)
        $matches = $existing.Length -eq $payload.Length
        for ($index = 0; $matches -and $index -lt $payload.Length; $index++) {
            if ($existing[$index] -ne $payload[$index]) {
                $matches = $false
            }
        }
        if (-not $matches) {
            throw 'CGB interest-tax RulePack binary is stale.'
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
