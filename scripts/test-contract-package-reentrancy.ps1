[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ficantRoot = Split-Path -Parent $PSScriptRoot
$packageRoot = Join-Path $ficantRoot 'web-dm\packages\contracts-generated'
$repositoryOutput = Join-Path $packageRoot 'dist'
$expectedPackagePath = Join-Path $repositoryOutput 'ficant-contracts-generated-0.0.0.tgz'
$packageScript = Join-Path $PSScriptRoot 'package-contracts.ps1'
$packageTestScript = Join-Path $PSScriptRoot 'test-contract-package.ps1'
$licenseInventoryPath = Join-Path $ficantRoot '.github\scripts\license-inventory.lock.json'
$expectedLicenseDigest = (Get-Content -LiteralPath $licenseInventoryPath -Raw | ConvertFrom-Json).inventory_digest
if ($expectedLicenseDigest -notmatch '^[0-9a-f]{64}$') {
    throw "License inventory contains an invalid digest '$expectedLicenseDigest'."
}

function Invoke-CapturedNative {
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,
        [Parameter(Mandatory)]
        [string[]]$ArgumentList,
        [Parameter(Mandatory)]
        [string]$WorkingDirectory
    )

    Push-Location -LiteralPath $WorkingDirectory
    try {
        $output = @(& $FilePath @ArgumentList 2>&1)
        $exitCode = $LASTEXITCODE
    }
    finally {
        Pop-Location
    }

    if ($exitCode -ne 0) {
        $renderedOutput = ($output | ForEach-Object { $_.ToString() }) -join [Environment]::NewLine
        throw "Native command failed with exit $exitCode`: $FilePath $($ArgumentList -join ' ')$([Environment]::NewLine)$renderedOutput"
    }

    return @($output | ForEach-Object { $_.ToString() })
}

function Remove-VerifiedRepositoryOutput {
    $expectedPath = [System.IO.Path]::GetFullPath(
        (Join-Path $ficantRoot 'web-dm\packages\contracts-generated\dist')
    )
    $candidatePath = [System.IO.Path]::GetFullPath($repositoryOutput)
    if (-not [string]::Equals($candidatePath, $expectedPath, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove unexpected repository output '$candidatePath'."
    }
    if (-not (Test-Path -LiteralPath $candidatePath)) {
        return
    }

    $resolvedPath = [System.IO.Path]::GetFullPath(
        (Resolve-Path -LiteralPath $candidatePath).ProviderPath
    )
    if (-not [string]::Equals($resolvedPath, $expectedPath, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove repository output resolved outside the expected path: '$resolvedPath'."
    }
    $item = Get-Item -LiteralPath $resolvedPath -Force
    if (-not $item.PSIsContainer) {
        throw "Refusing to remove repository output because it is not a directory: '$resolvedPath'."
    }
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing to remove repository output because it is a reparse point: '$resolvedPath'."
    }

    Remove-Item -LiteralPath $resolvedPath -Recurse -Force
}

$licenseArguments = @(
    'run',
    '--offline',
    '--locked',
    '--project',
    'python',
    'python',
    '.github/scripts/verify-license-inventory.py',
    'verify-bindings',
    '--inventory',
    '.github/scripts/license-inventory.lock.json',
    '--release-root',
    '.',
    '--cargo-lock',
    'Cargo.lock',
    '--uv-lock',
    'python/uv.lock',
    '--pnpm-lock',
    'web-dm/pnpm-lock.yaml',
    '--supply-lock',
    '.github/scripts/supply-chain.lock.json'
)

$testsPassed = 0
$contractTestsPassed = 0
try {
    $packageOutput = @(& $packageScript)
    if ($packageOutput.Count -ne 1) {
        throw "Expected one explicit package evidence record, received $($packageOutput.Count)."
    }
    $package = $packageOutput[0] | ConvertFrom-Json
    if (-not [string]::Equals(
        [System.IO.Path]::GetFullPath($package.package_path),
        [System.IO.Path]::GetFullPath($expectedPackagePath),
        [StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Explicit package output path is unexpected: '$($package.package_path)'."
    }
    if (-not (Test-Path -LiteralPath $expectedPackagePath -PathType Leaf)) {
        throw "Explicit packaging did not leave a consumable package at '$expectedPackagePath'."
    }
    $testsPassed++

    foreach ($run in 1..2) {
        $licenseOutput = @(Invoke-CapturedNative -FilePath 'uv' -ArgumentList $licenseArguments -WorkingDirectory $ficantRoot)
        $actualDigest = ($licenseOutput -join [Environment]::NewLine).Trim()
        if ($actualDigest -ne $expectedLicenseDigest) {
            throw "License binding run $run returned unexpected digest '$actualDigest'."
        }
        $testsPassed++
    }

    foreach ($run in 1..2) {
        $testOutput = @(& $packageTestScript -RemoveRepositoryOutput)
        if ($testOutput.Count -ne 1) {
            throw "Contract package test run $run returned $($testOutput.Count) records instead of one."
        }
        $testEvidence = $testOutput[0] | ConvertFrom-Json
        if ($testEvidence.tests_passed -ne 6) {
            throw "Contract package test run $run reported '$($testEvidence.tests_passed)' tests instead of 6."
        }
        $contractTestsPassed += $testEvidence.tests_passed
        $testsPassed++

        if (Test-Path -LiteralPath $repositoryOutput) {
            throw "Contract package test run $run left repository output '$repositoryOutput'."
        }
        $testsPassed++
    }

    [ordered]@{
        schema = 'ficant.contract-package-reentrancy-test-evidence.v1'
        tests_passed = $testsPassed
        contract_test_runs = 2
        contract_tests_passed = $contractTestsPassed
        license_binding_runs = 2
        explicit_package_path = [System.IO.Path]::GetFullPath($expectedPackagePath)
        repository_output_removed = -not (Test-Path -LiteralPath $repositoryOutput)
    } | ConvertTo-Json -Compress
}
finally {
    Remove-VerifiedRepositoryOutput
}
