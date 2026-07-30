[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Join-Path $PSScriptRoot '..')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:RepositoryRoot = [System.IO.Path]::GetFullPath($RepositoryRoot)
$allowlistPath = Join-Path $script:RepositoryRoot 'scripts\layering-allowlist.json'
$script:SourceExtensions = @(
    '.c', '.cc', '.cpp', '.cxx', '.h', '.hh', '.hpp', '.hxx',
    '.rs', '.proto', '.py', '.ps1', '.psm1', '.ts', '.tsx', '.js', '.jsx',
    '.json', '.yaml', '.yml', '.toml', '.sql'
)
$script:SourceRoots = @('.')
$script:CountryCode = [string]::Concat([char]67, [char]78)

function Get-RepositoryRelativePath {
    param([Parameter(Mandatory)][string]$Path)

    return [System.IO.Path]::GetRelativePath($script:RepositoryRoot, $Path).Replace('\', '/')
}

function Test-ExcludedSourcePath {
    param([Parameter(Mandatory)][string]$Path)

    $relativePath = Get-RepositoryRelativePath -Path $Path
    return $relativePath -match '(?i)(^|/)(\.git|target|build|node_modules|vendor|generated|fixtures|\.venv|venv|__pycache__|dist|coverage)(/|$)' -or
        $relativePath -match '(?i)(^|/)(Cargo\.lock|pnpm-lock\.yaml|uv\.lock)$'
}

function Get-ImplementationFiles {
    $files = foreach ($relativeRoot in $script:SourceRoots) {
        $absoluteRoot = Join-Path $script:RepositoryRoot $relativeRoot
        if (-not (Test-Path -LiteralPath $absoluteRoot -PathType Container)) {
            continue
        }

        Get-ChildItem -LiteralPath $absoluteRoot -File -Recurse -Force | Where-Object {
            $extension = [System.IO.Path]::GetExtension($_.Name).ToLowerInvariant()
            $extension -in $script:SourceExtensions -and -not (Test-ExcludedSourcePath -Path $_.FullName)
        }
    }

    return @($files)
}

function Read-LayeringAllowlist {
    if (-not (Test-Path -LiteralPath $allowlistPath -PathType Leaf)) {
        throw "Layering allowlist is missing: $allowlistPath"
    }

    $raw = (Get-Content -LiteralPath $allowlistPath -Raw).Trim()
    if ([string]::IsNullOrWhiteSpace($raw) -or -not $raw.StartsWith('[')) {
        throw 'Layering allowlist must be a JSON array.'
    }

    if ($raw -eq '[]') {
        $decodedEntries = @()
    }
    else {
        $decodedEntries = @($raw | ConvertFrom-Json -AsHashtable)
    }

    if ($decodedEntries.Count -ne 0) {
        throw 'Layering allowlist must be empty after the R2 ratchet; entries may only be removed.'
    }

    return @()
}

function Get-MarketBranchViolations {
    $patterns = [System.Collections.Generic.List[string]]::new()
    [void]$patterns.Add('(?i)\bmarket(?:\.[A-Za-z_][A-Za-z0-9_]*)?\s*(?:==|===|!=|!==)\s*["'']' + $script:CountryCode + '["'']')
    [void]$patterns.Add('(?i)["'']' + $script:CountryCode + '["'']\s*(?:==|===|!=|!==)\s*\bmarket(?:\.[A-Za-z_][A-Za-z0-9_]*)?')
    [void]$patterns.Add('(?i)\bmarket(?:\.[A-Za-z_][A-Za-z0-9_]*)?\s*(?:==|===|!=|!==)\s*(?:Market::)?' + $script:CountryCode + '\b')
    [void]$patterns.Add('(?i)\b(?:Market::)?' + $script:CountryCode + '\s*=>')
    [void]$patterns.Add('(?i)["'']' + $script:CountryCode + '["'']\s*=>')
    [void]$patterns.Add('(?i)\bcase\s+["'']' + $script:CountryCode + '["'']')
    [void]$patterns.Add('(?i)["'']' + $script:CountryCode + '["'']\s*:')
    [void]$patterns.Add('(?i)\(\s*["'']' + $script:CountryCode + '["'']\s*,')
    [void]$patterns.Add('(?i)["'']C["'']\s*\+\s*["'']N["'']')

    $violations = foreach ($file in @(Get-ImplementationFiles)) {
        $lineNumber = 0
        foreach ($line in Get-Content -LiteralPath $file.FullName) {
            $lineNumber++
            foreach ($pattern in $patterns) {
                if ($line -match $pattern) {
                    [pscustomobject]@{
                        Path = Get-RepositoryRelativePath -Path $file.FullName
                        Line = $lineNumber
                        Category = 'market-branch'
                        Text = $line.Trim()
                    }
                    break
                }
            }
        }
    }

    return @($violations)
}

function Get-DomainRuleViolations {
    $domainRoot = Join-Path $script:RepositoryRoot 'crates\ficant-domain\src'
    if (-not (Test-Path -LiteralPath $domainRoot -PathType Container)) {
        throw "Domain source root is missing: $domainRoot"
    }

    $patterns = @(
        [pscustomobject]@{ Category = 'term-months-table'; Pattern = '(?i)\b(?:original_term_months|residual_term_bounds)\b' },
        [pscustomobject]@{ Category = 'residual-term-range'; Pattern = '(?i)\(\s*(?:18|48|78|300)\s*,\s*(?:Some\(\s*(?:27|63)\s*\)|None)\s*\)' },
        [pscustomobject]@{ Category = 'tax-rate'; Pattern = '(?i)\b(?:tax|vat)[A-Za-z_]*rate\b\s*(?:=|:)\s*[-+]?\d+(?:\.\d+)?' },
        [pscustomobject]@{ Category = 'conversion-factor'; Pattern = '(?i)\bconversion[_ -]*factor\b\s*(?:=|:)\s*[-+]?\d+(?:\.\d+)?' }
    )

    $violations = foreach ($file in @(Get-ChildItem -LiteralPath $domainRoot -File -Recurse | Where-Object {
            [System.IO.Path]::GetExtension($_.Name).ToLowerInvariant() -in @('.rs', '.py', '.ts', '.tsx') -and
            -not (Test-ExcludedSourcePath -Path $_.FullName)
        })) {
        $lineNumber = 0
        foreach ($line in Get-Content -LiteralPath $file.FullName) {
            $lineNumber++
            foreach ($rule in $patterns) {
                if ($line -match $rule.Pattern) {
                    [pscustomobject]@{
                        Path = Get-RepositoryRelativePath -Path $file.FullName
                        Line = $lineNumber
                        Category = $rule.Category
                        Text = $line.Trim()
                    }
                    break
                }
            }
        }
    }

    return @($violations)
}

function Get-Phase2CProductionRuleValueViolations {
    $candidatePaths = [System.Collections.Generic.List[string]]::new()
    $cppSourceRoot = Join-Path $script:RepositoryRoot 'cpp\fixed-income-kernel\src'
    if (Test-Path -LiteralPath $cppSourceRoot -PathType Container) {
        Get-ChildItem -LiteralPath $cppSourceRoot -File | Where-Object {
            $_.BaseName -like 'futures_*' -and $_.Extension -in @('.cpp', '.hpp', '.h')
        } | ForEach-Object {
            $candidatePaths.Add((Get-RepositoryRelativePath -Path $_.FullName))
        }
    }
    foreach ($relativePath in @(
            'cpp/fixed-income-kernel/include/ficant_kernel.h',
            'crates/ficant-kernel-sys/src/lib.rs',
            'crates/ficant-fixed-income-native/src/lib.rs'
        )) {
        $candidatePaths.Add($relativePath)
    }
    $patterns = @(
        [pscustomobject]@{
            Category = 'eligibility-bounds'
            Pattern = '(?i)\b(?:original_term_max_months|residual_min_months|residual_max_months)\b\s*(?:=|:)\s*\d+'
        },
        [pscustomobject]@{
            Category = 'delivery-months'
            Pattern = '(?i)\bdelivery_months\b[^\r\n]*[=:\{][^\r\n]*\b(?:3|6|9|12)\b'
        },
        [pscustomobject]@{
            Category = 'standard-coupon'
            Pattern = '(?i)\b(?:nominal_coupon|standard_coupon)\b\s*(?:=|:)\s*(?:\d+\.\d+|\.\d+)'
        },
        [pscustomobject]@{
            Category = 'face-quote-basis'
            Pattern = '(?i)\b(?:face_quote_basis|face_per_hundred)\b\s*(?:=|:)\s*100(?:\.0+)?\b'
        },
        [pscustomobject]@{
            Category = 'rounding-scale'
            Pattern = '(?i)\b(?:conversion_factor_rounding_places|accrued_interest_rounding_places)\b\s*(?:=|:)\s*\d+'
        },
        [pscustomobject]@{
            Category = 'annual-day-basis'
            Pattern = '(?i)\bannual_day_basis\b\s*(?:=|:)\s*\d+'
        }
    )

    $violations = foreach ($relativePath in $candidatePaths | Sort-Object -Unique) {
        $path = Join-Path $script:RepositoryRoot ($relativePath.Replace('/', '\\'))
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            continue
        }
        $lineNumber = 0
        foreach ($line in Get-Content -LiteralPath $path) {
            $lineNumber++
            foreach ($rule in $patterns) {
                if ($line -match $rule.Pattern) {
                    [pscustomobject]@{
                        Path = $relativePath
                        Line = $lineNumber
                        Category = $rule.Category
                        Text = $line.Trim()
                    }
                    break
                }
            }
        }
    }

    return @($violations)
}

function Get-FundingRuleValueViolations {
    $candidatePaths = [System.Collections.Generic.List[string]]::new()
    foreach ($relativePath in @(
            'interface/proto/ficant/core/v1/subject.proto',
            'interface/proto/ficant/core/v1/subject_state.proto',
            'crates/ficant-domain/src/subject.rs',
            'crates/ficant-domain/src/subject_state.rs',
            'crates/ficant-kernel-sys/src/lib.rs',
            'crates/ficant-fixed-income-native/src/lib.rs'
        )) {
        $candidatePaths.Add($relativePath)
    }
    $cppRoot = Join-Path $script:RepositoryRoot 'cpp\fixed-income-kernel'
    if (Test-Path -LiteralPath $cppRoot -PathType Container) {
        Get-ChildItem -LiteralPath $cppRoot -File -Recurse | Where-Object {
            $_.Extension.ToLowerInvariant() -in @('.c', '.cc', '.cpp', '.cxx', '.h', '.hh', '.hpp', '.hxx')
        } | ForEach-Object {
            $candidatePaths.Add((Get-RepositoryRelativePath -Path $_.FullName))
        }
    }

    $pattern = '(?i)\b(?:annual_)?(?:funding|financing)[A-Za-z_]*rate\b\s*(?:=|:)\s*[-+]?(?:\d+\.\d+|\.\d+|\d+)'
    $violations = foreach ($relativePath in $candidatePaths | Sort-Object -Unique) {
        $path = Join-Path $script:RepositoryRoot ($relativePath.Replace('/', '\\'))
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            continue
        }
        $lineNumber = 0
        foreach ($line in Get-Content -LiteralPath $path) {
            $lineNumber++
            if ($line -match $pattern) {
                [pscustomobject]@{
                    Path = $relativePath
                    Line = $lineNumber
                    Category = 'funding-rate-value'
                    Text = $line.Trim()
                }
            }
        }
    }

    return @($violations)
}

function Get-TaxRuleValueViolations {
    $candidatePaths = [System.Collections.Generic.List[string]]::new()
    foreach ($relativePath in @(
            'interface/proto/ficant/core/v1/subject.proto',
            'interface/proto/ficant/core/v1/subject_state.proto',
            'crates/ficant-domain/src/subject.rs',
            'crates/ficant-domain/src/market/bond.rs',
            'crates/ficant-kernel-sys/src/lib.rs',
            'crates/ficant-fixed-income-native/src/lib.rs'
        )) {
        $candidatePaths.Add($relativePath)
    }
    $cppRoot = Join-Path $script:RepositoryRoot 'cpp\fixed-income-kernel'
    if (Test-Path -LiteralPath $cppRoot -PathType Container) {
        Get-ChildItem -LiteralPath $cppRoot -File -Recurse | Where-Object {
            $_.Extension.ToLowerInvariant() -in @('.c', '.cc', '.cpp', '.cxx', '.h', '.hh', '.hpp', '.hxx')
        } | ForEach-Object {
            $candidatePaths.Add((Get-RepositoryRelativePath -Path $_.FullName))
        }
    }

    $codePattern = '(?i)\b(?:coupon_)?(?:tax|vat)[A-Za-z_]*rate\b\s*(?:=|:)\s*[-+]?(?:\d+\.\d+|\.\d+|\d+)'
    $protoPattern = '(?i)\b(?:double|float)\s+(?:coupon_)?(?:tax|vat)[A-Za-z_]*rate\b\s*=\s*[-+]?(?:\d+\.\d+|\.\d+)'
    $violations = foreach ($relativePath in $candidatePaths | Sort-Object -Unique) {
        $path = Join-Path $script:RepositoryRoot ($relativePath.Replace('/', '\\'))
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            continue
        }
        $isProto = [System.IO.Path]::GetExtension($path).ToLowerInvariant() -eq '.proto'
        $pattern = if ($isProto) { $protoPattern } else { $codePattern }
        $lineNumber = 0
        foreach ($line in Get-Content -LiteralPath $path) {
            $lineNumber++
            if ($line -match $pattern) {
                [pscustomobject]@{
                    Path = $relativePath
                    Line = $lineNumber
                    Category = 'tax-rate-value'
                    Text = $line.Trim()
                }
            }
        }
    }

    return @($violations)
}

try {
    if (-not (Test-Path -LiteralPath $script:RepositoryRoot -PathType Container)) {
        throw "Repository root is missing: $script:RepositoryRoot"
    }

    $allowlist = @(Read-LayeringAllowlist)
    $marketBranchViolations = @(Get-MarketBranchViolations)
    if ($marketBranchViolations.Count -gt 0) {
        foreach ($violation in $marketBranchViolations) {
            Write-Error ("AC03 market branch at {0}:{1}: {2}" -f $violation.Path, $violation.Line, $violation.Text)
        }
        throw "Layering gate failed AC03 with $($marketBranchViolations.Count) violation(s)."
    }

    $domainRuleViolations = @(Get-DomainRuleViolations)
    if ($domainRuleViolations.Count -gt 0) {
        foreach ($violation in $domainRuleViolations) {
            Write-Error ("AC01 domain rule value at {0}:{1} ({2}): {3}" -f $violation.Path, $violation.Line, $violation.Category, $violation.Text)
        }
        throw "Layering gate failed AC01 with $($domainRuleViolations.Count) domain rule value violation(s)."
    }

    $productionRuleViolations = @(Get-Phase2CProductionRuleValueViolations)
    if ($productionRuleViolations.Count -gt 0) {
        foreach ($violation in $productionRuleViolations) {
            Write-Error ("AC01 Phase 2C production rule value at {0}:{1} ({2}): {3}" -f $violation.Path, $violation.Line, $violation.Category, $violation.Text)
        }
        throw "Layering gate failed AC01 with $($productionRuleViolations.Count) Phase 2C production C++/FFI rule value violation(s)."
    }

    $fundingRuleViolations = @(Get-FundingRuleValueViolations)
    if ($fundingRuleViolations.Count -gt 0) {
        foreach ($violation in $fundingRuleViolations) {
            Write-Error ("R3a funding rule value at {0}:{1} ({2}): {3}" -f $violation.Path, $violation.Line, $violation.Category, $violation.Text)
        }
        throw "Layering gate failed R3a with $($fundingRuleViolations.Count) Funding rule value violation(s)."
    }

    $taxRuleViolations = @(Get-TaxRuleValueViolations)
    if ($taxRuleViolations.Count -gt 0) {
        foreach ($violation in $taxRuleViolations) {
            Write-Error ("R3b tax rule value at {0}:{1} ({2}): {3}" -f $violation.Path, $violation.Line, $violation.Category, $violation.Text)
        }
        throw "Layering gate failed R3b with $($taxRuleViolations.Count) Tax rule value violation(s)."
    }

    Write-Host ("Layering gate passed: AC03=0 market branches; AC01=0 domain rule values; Phase2C production C++/FFI rule values=0; R3a Funding rule values=0; R3b Tax rule values=0; allowlist=$($allowlist.Count).")
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
