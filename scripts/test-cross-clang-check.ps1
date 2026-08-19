[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$gatePath = Join-Path $PSScriptRoot 'check-cross-clang.ps1'
$tempBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$tempRoot = [System.IO.Path]::GetFullPath((Join-Path $tempBase ('ficant-cross-clang-fixtures-' + [Guid]::NewGuid().ToString('N'))))
if (-not $tempRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to create fixtures outside the temporary directory: $tempRoot"
}

function Invoke-Comparator {
    param(
        [Parameter(Mandatory)][string]$WindowsOutput,
        [Parameter(Mandatory)][string]$LinuxOutput,
        [Parameter(Mandatory)][int]$ExpectedExitCode,
        [Parameter(Mandatory)][string]$Scenario
    )

    $output = & pwsh -NoProfile -NonInteractive -File $gatePath -CompareOnly `
        -WindowsOutput $WindowsOutput -LinuxOutput $LinuxOutput 2>&1
    $actualExitCode = $LASTEXITCODE
    if ($actualExitCode -ne $ExpectedExitCode) {
        throw "Scenario '$Scenario' expected exit code $ExpectedExitCode but got $actualExitCode.`n$($output -join [Environment]::NewLine)"
    }
}

try {
    New-Item -ItemType Directory -Path $tempRoot | Out-Null
    $windowsOutput = Join-Path $tempRoot 'windows.tsv'
    $matchingLinuxOutput = Join-Path $tempRoot 'linux-matching.tsv'
    $driftedLinuxOutput = Join-Path $tempRoot 'linux-drifted.tsv'
    $baseline = @(
        "i.bond.status`t0",
        "i.bond.cashflow_count`t2",
        "f.bond.clean_price`t4059000000000000"
    )
    [System.IO.File]::WriteAllLines($windowsOutput, $baseline)
    [System.IO.File]::WriteAllLines($matchingLinuxOutput, $baseline)
    [System.IO.File]::WriteAllLines($driftedLinuxOutput, @(
            "i.bond.status`t0",
            "i.bond.cashflow_count`t2",
            "f.bond.clean_price`t4059000000000001"
        ))

    Invoke-Comparator -WindowsOutput $windowsOutput -LinuxOutput $matchingLinuxOutput `
        -ExpectedExitCode 0 -Scenario 'identical typed manifests pass'
    Invoke-Comparator -WindowsOutput $windowsOutput -LinuxOutput $driftedLinuxOutput `
        -ExpectedExitCode 1 -Scenario 'one IEEE-754 bit of drift fails'

    Write-Host 'Cross-Clang comparator fixture tests passed (2 assertions).'
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
finally {
    if (Test-Path -LiteralPath $tempRoot -PathType Container) {
        $resolvedRoot = (Resolve-Path -LiteralPath $tempRoot).Path
        if (-not $resolvedRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase) -or
            (Split-Path -Leaf $resolvedRoot) -notmatch '^ficant-cross-clang-fixtures-[0-9a-f]{32}$') {
            throw "Refusing to remove unexpected fixture root: $resolvedRoot"
        }
        Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
    }
}
