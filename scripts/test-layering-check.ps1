[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$gatePath = Join-Path $PSScriptRoot 'check-layering.ps1'
$fixtureProducerPath = Join-Path $repoRoot 'python\tests\test_layering_invariants.py'

if (-not (Test-Path -LiteralPath $gatePath -PathType Leaf)) {
    throw "Layering gate script is missing: $gatePath"
}
if (-not (Test-Path -LiteralPath $fixtureProducerPath -PathType Leaf)) {
    throw "Shared layering fixture producer is missing: $fixtureProducerPath"
}
if ($null -eq (Get-Command 'pwsh' -ErrorAction SilentlyContinue)) {
    throw 'Required command pwsh was not found.'
}
if ($null -eq (Get-Command 'uv' -ErrorAction SilentlyContinue)) {
    throw 'Required command uv was not found.'
}

$tempBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$tempRoot = [System.IO.Path]::GetFullPath((Join-Path $tempBase ('ficant-layering-gate-' + [Guid]::NewGuid().ToString('N'))))
if (-not $tempRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to create fixture outside the temporary directory: $tempRoot"
}

function Write-FixtureFile {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string]$RelativePath,
        [Parameter(Mandatory)][string]$Content
    )

    $path = Join-Path $Root ($RelativePath.Replace('/', '\'))
    $parent = Split-Path -Parent $path
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    [System.IO.File]::WriteAllText($path, $Content, [System.Text.UTF8Encoding]::new($false))
    return $path
}

function Initialize-CleanFixtureRoot {
    param([Parameter(Mandatory)][string]$Root)

    New-Item -ItemType Directory -Path $Root -Force | Out-Null
    Write-FixtureFile -Root $Root -RelativePath 'scripts/layering-allowlist.json' -Content "[]`n" | Out-Null
    Write-FixtureFile -Root $Root -RelativePath 'crates/ficant-domain/src/placeholder.rs' -Content "pub const CLEAN: u32 = 1;`n" | Out-Null
}

try {
    Push-Location -LiteralPath $repoRoot
    try {
        $uvArgs = @(
            'run', '--locked', '--project', 'python',
            'python', 'python/tests/test_layering_invariants.py', '--emit-fixtures'
        )
        $rawJsonLines = & uv @uvArgs
        $uvExitCode = $LASTEXITCODE
    }
    finally {
        Pop-Location
    }

    if ($uvExitCode -ne 0) {
        throw "Failed to emit shared layering fixtures via uv (exit code $uvExitCode)."
    }

    $rawJson = ($rawJsonLines -join "`n").Trim()
    if ([string]::IsNullOrWhiteSpace($rawJson)) {
        throw 'Shared layering fixture emitter returned empty output.'
    }

    $catalog = $rawJson | ConvertFrom-Json -Depth 10
    if ($null -eq $catalog -or $catalog.version -ne 1) {
        throw "Unsupported shared layering fixture catalog version: $($catalog.version)"
    }

    $fixtures = @($catalog.fixtures)
    if ($fixtures.Count -eq 0) {
        throw 'Shared layering fixture catalog is empty.'
    }

    $seenIds = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($fixture in $fixtures) {
        if ([string]::IsNullOrWhiteSpace($fixture.id)) {
            throw 'Fixture entry is missing a non-empty id.'
        }
        if (-not $seenIds.Add($fixture.id)) {
            throw "Duplicate fixture id detected in shared catalog: $($fixture.id)"
        }
        if ($fixture.expected_exit_code -notin @(0, 1)) {
            throw "Fixture '$($fixture.id)' has invalid expected_exit_code: $($fixture.expected_exit_code)"
        }
    }

    $blotterStoreFixtures = @($fixtures | Where-Object {
        ([string]$_.relative_path).StartsWith('crates/ficant-blotter-store/src/', [System.StringComparison]::Ordinal)
    })
    $blotterStorePositive = @($blotterStoreFixtures | Where-Object { [int]$_.expected_exit_code -eq 0 })
    $blotterStoreNegative = @($blotterStoreFixtures | Where-Object { [int]$_.expected_exit_code -eq 1 })
    if ($blotterStorePositive.Count -lt 1 -or $blotterStoreNegative.Count -lt 3) {
        throw "Shared layering fixture catalog must cover 8th production root crates/ficant-blotter-store/src with >=1 positive and >=3 negative cases (got positive=$($blotterStorePositive.Count), negative=$($blotterStoreNegative.Count))."
    }

    New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null

    $preparedFixtures = foreach ($fixture in $fixtures) {
        $fixtureRoot = Join-Path $tempRoot $fixture.id
        Initialize-CleanFixtureRoot -Root $fixtureRoot
        Write-FixtureFile -Root $fixtureRoot -RelativePath $fixture.relative_path -Content $fixture.content | Out-Null
        [pscustomobject]@{
            Id = [string]$fixture.id
            Category = [string]$fixture.category
            RelativePath = [string]$fixture.relative_path
            ExpectedExitCode = [int]$fixture.expected_exit_code
            FixtureRoot = $fixtureRoot
        }
    }

    $results = @($preparedFixtures | ForEach-Object -Parallel {
        $item = $_
        $output = & pwsh -NoProfile -NonInteractive -File $using:gatePath -RepositoryRoot $item.FixtureRoot 2>&1
        $actualExitCode = $LASTEXITCODE
        [pscustomobject]@{
            Id = $item.Id
            Category = $item.Category
            RelativePath = $item.RelativePath
            ExpectedExitCode = $item.ExpectedExitCode
            ActualExitCode = $actualExitCode
            Output = ($output | Out-String).Trim()
        }
    } -ThrottleLimit 8)

    $resultsById = @{}
    foreach ($res in $results) {
        $resultsById[$res.Id] = $res
    }

    if ($resultsById.Count -ne $seenIds.Count) {
        throw "Fixture result count mismatch: expected $($seenIds.Count), got $($resultsById.Count)."
    }

    $assertionCount = 0
    $positiveCount = 0
    $negativeCount = 0
    $failures = [System.Collections.Generic.List[string]]::new()

    foreach ($fixture in $fixtures) {
        $fid = [string]$fixture.id
        if (-not $resultsById.ContainsKey($fid)) {
            $failures.Add("Missing execution result for fixture '$fid'.")
            continue
        }

        $res = $resultsById[$fid]
        if ($res.ActualExitCode -ne $res.ExpectedExitCode) {
            $failures.Add(
                "Fixture '$fid' ($($res.RelativePath)) expected exit code $($res.ExpectedExitCode) but got $($res.ActualExitCode). Output: $($res.Output)"
            )
            continue
        }

        $assertionCount++
        if ($res.ExpectedExitCode -eq 0) {
            $positiveCount++
        }
        else {
            $negativeCount++
        }
    }

    if ($failures.Count -gt 0) {
        foreach ($failure in $failures) {
            Write-Error $failure
        }
        throw "Shared layering fixture verification failed with $($failures.Count) mismatch(es)."
    }

    # Verify removal-restores-pass lifecycle in an isolated temp root.
    $restoreRoot = Join-Path $tempRoot '_lifecycle_restore'
    Initialize-CleanFixtureRoot -Root $restoreRoot
    $countryCode = [string]::Concat([char]67, [char]78)
    $injectedPath = Write-FixtureFile -Root $restoreRoot -RelativePath 'crates/ficant-domain/src/restore_check.rs' -Content ('let flag = market == "' + $countryCode + '";')
    $out1 = & pwsh -NoProfile -NonInteractive -File $gatePath -RepositoryRoot $restoreRoot 2>&1
    if ($LASTEXITCODE -ne 1) {
        throw "Lifecycle restore check expected exit 1 after injection, got $LASTEXITCODE. Output: $(($out1 | Out-String).Trim())"
    }
    $assertionCount++
    Remove-Item -LiteralPath $injectedPath
    $out2 = & pwsh -NoProfile -NonInteractive -File $gatePath -RepositoryRoot $restoreRoot 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Lifecycle restore check expected exit 0 after removal, got $LASTEXITCODE. Output: $(($out2 | Out-String).Trim())"
    }
    $assertionCount++

    Write-Host (
        "Layering gate shared fixture tests passed: fixtures={0} (positive={1}, negative={2}, blotter_store_8th_root={3}), assertions={4}." -f
        $fixtures.Count, $positiveCount, $negativeCount, $blotterStoreFixtures.Count, $assertionCount
    )
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
finally {
    if (Test-Path -LiteralPath $tempRoot -PathType Container) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
}
