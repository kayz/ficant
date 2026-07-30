[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$gatePath = Join-Path $PSScriptRoot 'check-layering.ps1'
if (-not (Test-Path -LiteralPath $gatePath -PathType Leaf)) {
    throw "Layering gate script is missing: $gatePath"
}
if ($null -eq (Get-Command 'pwsh' -ErrorAction SilentlyContinue)) {
    throw 'Required command pwsh was not found.'
}

$script:AssertionCount = 0

$tempBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$tempRoot = [System.IO.Path]::GetFullPath((Join-Path $tempBase ('ficant-layering-gate-' + [Guid]::NewGuid().ToString('N'))))
if (-not $tempRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to create fixture outside the temporary directory: $tempRoot"
}

function Write-FixtureFile {
    param(
        [Parameter(Mandatory)][string]$RelativePath,
        [Parameter(Mandatory)][string]$Content
    )

    $path = Join-Path $tempRoot ($RelativePath.Replace('/', '\'))
    $parent = Split-Path -Parent $path
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    Set-Content -LiteralPath $path -Value $Content -Encoding utf8
    return $path
}

function Invoke-GateExpect {
    param(
        [Parameter(Mandatory)][int]$ExpectedExitCode,
        [Parameter(Mandatory)][string]$Scenario
    )

    $output = & pwsh -NoProfile -File $gatePath -RepositoryRoot $tempRoot 2>&1
    $actualExitCode = $LASTEXITCODE
    if ($actualExitCode -ne $ExpectedExitCode) {
        $renderedOutput = ($output | Out-String).Trim()
        throw "Scenario '$Scenario' expected exit code $ExpectedExitCode but got $actualExitCode. Output: $renderedOutput"
    }

    $script:AssertionCount++
}

$countryCode = [string]::Concat([char]67, [char]78)
$allowlist = '[]'

try {
    New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null
    Write-FixtureFile -RelativePath 'scripts/layering-allowlist.json' -Content $allowlist | Out-Null
    Write-FixtureFile -RelativePath 'crates/ficant-domain/src/placeholder.rs' -Content 'pub const CLEAN: u32 = 1;' | Out-Null
    Invoke-GateExpect -ExpectedExitCode 0 -Scenario 'empty allowlist and clean source pass'

    $domainRulePath = Write-FixtureFile -RelativePath 'crates/ficant-domain/src/market_rules.rs' -Content 'fn original_term_months() -> u32 { 120 }'
    Invoke-GateExpect -ExpectedExitCode 1 -Scenario 'domain rule table is rejected'
    Remove-Item -LiteralPath $domainRulePath
    Invoke-GateExpect -ExpectedExitCode 0 -Scenario 'domain rule table removal restores pass'

    $comparison = 'let supported = market == "' + $countryCode + '";'
    $comparisonPath = Write-FixtureFile -RelativePath 'crates/ficant-domain/src/market_comparison.rs' -Content $comparison
    Invoke-GateExpect -ExpectedExitCode 1 -Scenario 'market comparison is rejected'
    Remove-Item -LiteralPath $comparisonPath
    Invoke-GateExpect -ExpectedExitCode 0 -Scenario 'market comparison removal restores pass'

    $matchBranch = 'match market { "' + $countryCode + '" => 1, _ => 0 }'
    $matchPath = Write-FixtureFile -RelativePath 'crates/ficant-domain/src/market_match.rs' -Content $matchBranch
    Invoke-GateExpect -ExpectedExitCode 1 -Scenario 'market match branch is rejected'
    Remove-Item -LiteralPath $matchPath
    Invoke-GateExpect -ExpectedExitCode 0 -Scenario 'market match removal restores pass'

    $mapBranch = 'let values = HashMap::from([("' + $countryCode + '", 1)]);'
    $mapPath = Write-FixtureFile -RelativePath 'crates/ficant-domain/src/market_map.rs' -Content $mapBranch
    Invoke-GateExpect -ExpectedExitCode 1 -Scenario 'market keyed map is rejected'
    Remove-Item -LiteralPath $mapPath
    Invoke-GateExpect -ExpectedExitCode 0 -Scenario 'market map removal restores pass'

    $quote = [char]34
    $concatenatedBranch = 'let supported = market == (' + $quote + [char]67 + $quote + ' + ' + $quote + [char]78 + $quote + ');'
    $concatenatedPath = Write-FixtureFile -RelativePath 'crates/ficant-domain/src/market_concatenation.rs' -Content $concatenatedBranch
    Invoke-GateExpect -ExpectedExitCode 1 -Scenario 'concatenated market code is rejected'
    Remove-Item -LiteralPath $concatenatedPath
    Invoke-GateExpect -ExpectedExitCode 0 -Scenario 'concatenated market code removal restores pass'

    $cppPath = Write-FixtureFile -RelativePath 'cpp/market_branch.cpp' -Content ('if (market == "' + $countryCode + '") { return 1; }')
    Invoke-GateExpect -ExpectedExitCode 1 -Scenario 'C++ market branch is rejected'
    Remove-Item -LiteralPath $cppPath
    Invoke-GateExpect -ExpectedExitCode 0 -Scenario 'C++ market branch removal restores pass'

    $cppRuleFixtures = @(
        [pscustomobject]@{ Scenario = 'C++ delivery eligibility rule'; Content = 'static const uint32_t original_term_max_months = 120;' },
        [pscustomobject]@{ Scenario = 'C++ delivery months rule'; Content = 'static const uint32_t delivery_months[] = {3, 6, 9, 12};' },
        [pscustomobject]@{ Scenario = 'C++ standard coupon rule'; Content = 'static const double nominal_coupon = 0.03;' },
        [pscustomobject]@{ Scenario = 'C++ face quote basis rule'; Content = 'static const double face_quote_basis = 100.0;' },
        [pscustomobject]@{ Scenario = 'C++ rounding scale rule'; Content = 'static const uint32_t conversion_factor_rounding_places = 4;' },
        [pscustomobject]@{ Scenario = 'C++ annual day basis rule'; Content = 'static const uint32_t annual_day_basis = 365;' }
    )
    foreach ($fixture in $cppRuleFixtures) {
        $cppRulePath = Write-FixtureFile -RelativePath 'cpp/fixed-income-kernel/src/futures_rules.cpp' -Content $fixture.Content
        Invoke-GateExpect -ExpectedExitCode 1 -Scenario ($fixture.Scenario + ' is rejected')
        Remove-Item -LiteralPath $cppRulePath
        Invoke-GateExpect -ExpectedExitCode 0 -Scenario ($fixture.Scenario + ' removal restores pass')
    }

    $fundingRuleFixtures = @(
        [pscustomobject]@{ Scenario = 'Subject funding rate'; Path = 'interface/proto/ficant/core/v1/subject.proto'; Content = 'double annual_financing_rate = 0.018;' },
        [pscustomobject]@{ Scenario = 'SubjectState funding rate'; Path = 'interface/proto/ficant/core/v1/subject_state.proto'; Content = 'double financing_rate = 0.018;' },
        [pscustomobject]@{ Scenario = 'domain funding rate'; Path = 'crates/ficant-domain/src/subject.rs'; Content = 'let funding_rate = 0.018;' },
        [pscustomobject]@{ Scenario = 'C++ funding rate'; Path = 'cpp/fixed-income-kernel/src/funding.cpp'; Content = 'const double financing_rate = 0.018;' },
        [pscustomobject]@{ Scenario = 'FFI funding rate'; Path = 'crates/ficant-kernel-sys/src/lib.rs'; Content = 'let annual_financing_rate = 0.018;' }
    )
    foreach ($fixture in $fundingRuleFixtures) {
        $fundingRulePath = Write-FixtureFile -RelativePath $fixture.Path -Content $fixture.Content
        Invoke-GateExpect -ExpectedExitCode 1 -Scenario ($fixture.Scenario + ' is rejected')
        Remove-Item -LiteralPath $fundingRulePath
        Invoke-GateExpect -ExpectedExitCode 0 -Scenario ($fixture.Scenario + ' removal restores pass')
    }

    $taxRuleFixtures = @(
        [pscustomobject]@{ Scenario = 'Subject tax rate'; Path = 'interface/proto/ficant/core/v1/subject.proto'; Content = 'double coupon_tax_rate = 0.13;' },
        [pscustomobject]@{ Scenario = 'Bond/domain tax rate'; Path = 'crates/ficant-domain/src/market/bond.rs'; Content = 'let coupon_tax_rate = 0.13;' },
        [pscustomobject]@{ Scenario = 'C++ tax rate'; Path = 'cpp/fixed-income-kernel/src/tax.cpp'; Content = 'const double coupon_tax_rate = 0.13;' },
        [pscustomobject]@{ Scenario = 'FFI tax rate'; Path = 'crates/ficant-kernel-sys/src/lib.rs'; Content = 'let coupon_tax_rate = 0.13;' }
    )
    foreach ($fixture in $taxRuleFixtures) {
        $taxRulePath = Write-FixtureFile -RelativePath $fixture.Path -Content $fixture.Content
        Invoke-GateExpect -ExpectedExitCode 1 -Scenario ($fixture.Scenario + ' is rejected')
        Remove-Item -LiteralPath $taxRulePath
        Invoke-GateExpect -ExpectedExitCode 0 -Scenario ($fixture.Scenario + ' removal restores pass')
    }

    $testPath = Write-FixtureFile -RelativePath 'tests/market_branch.rs' -Content ('match market { "' + $countryCode + '" => 1, _ => 0 }')
    Invoke-GateExpect -ExpectedExitCode 1 -Scenario 'test source market branch is rejected'
    Remove-Item -LiteralPath $testPath
    Invoke-GateExpect -ExpectedExitCode 0 -Scenario 'test source market branch removal restores pass'

    $migrationPath = Write-FixtureFile -RelativePath 'migrations/market_branch.sql' -Content ('CASE ''' + $countryCode + ''' WHEN ''x'' THEN 1 END')
    Invoke-GateExpect -ExpectedExitCode 1 -Scenario 'migration market branch is rejected'
    Remove-Item -LiteralPath $migrationPath
    Invoke-GateExpect -ExpectedExitCode 0 -Scenario 'migration market branch removal restores pass'

    $unallowlistedRulePath = Write-FixtureFile -RelativePath 'crates/ficant-domain/src/other_market_rules.rs' -Content 'fn residual_term_bounds() -> (u32, Option<u32>) { (48, Some(63)) }'
    Invoke-GateExpect -ExpectedExitCode 1 -Scenario 'second domain rule table is rejected'
    Remove-Item -LiteralPath $unallowlistedRulePath
    Invoke-GateExpect -ExpectedExitCode 0 -Scenario 'second domain rule removal restores pass'

    Write-FixtureFile -RelativePath 'scripts/layering-allowlist.json' -Content '[{"path":"crates/ficant-domain/src/futures_delivery.rs","violation":"market-rule-values","removal_round":"R2"}]' | Out-Null
    Invoke-GateExpect -ExpectedExitCode 1 -Scenario 'nonempty allowlist is rejected'
    Write-FixtureFile -RelativePath 'scripts/layering-allowlist.json' -Content '[]' | Out-Null
    Invoke-GateExpect -ExpectedExitCode 0 -Scenario 'empty allowlist restores pass'

    Write-Host ("Layering gate fixture tests passed ({0} assertions)." -f $script:AssertionCount)
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
