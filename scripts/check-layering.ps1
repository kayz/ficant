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
$script:AllowedAllowlistKey = 'crates/ficant-domain/src/futures_delivery.rs|market-rule-values|R2'

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

    $entries = [System.Collections.Generic.List[object]]::new()
    $keys = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($entry in $decodedEntries) {
        if ($entry -isnot [System.Collections.IDictionary]) {
            throw 'Every layering allowlist entry must be an object.'
        }

        $actualProperties = @($entry.Keys | ForEach-Object { [string]$_ })
        $expectedProperties = @('path', 'violation', 'removal_round')
        $missingProperties = @($expectedProperties | Where-Object { $_ -notin $actualProperties })
        $unknownProperties = @($actualProperties | Where-Object { $_ -notin $expectedProperties })
        if ($missingProperties.Count -gt 0 -or $unknownProperties.Count -gt 0 -or $actualProperties.Count -ne 3) {
            throw "Layering allowlist entries must contain exactly path, violation, removal_round; missing=$($missingProperties -join ',') unknown=$($unknownProperties -join ',')."
        }

        $relativePath = ([string]$entry['path']).Trim().Replace('\', '/')
        while ($relativePath.StartsWith('./', [System.StringComparison]::Ordinal)) {
            $relativePath = $relativePath.Substring(2)
        }
        $violation = ([string]$entry['violation']).Trim()
        $removalRound = ([string]$entry['removal_round']).Trim()
        if ([string]::IsNullOrWhiteSpace($relativePath) -or [System.IO.Path]::IsPathRooted($relativePath) -or
            [string]::IsNullOrWhiteSpace($violation) -or [string]::IsNullOrWhiteSpace($removalRound)) {
            throw 'Layering allowlist entries must use non-empty relative path, violation, and removal_round values.'
        }

        $key = '{0}|{1}|{2}' -f $relativePath, $violation, $removalRound
        if (-not $keys.Add($key)) {
            throw "Duplicate layering allowlist entry: $key"
        }
        if ($key -ne $script:AllowedAllowlistKey) {
            throw "Unauthorized layering allowlist entry: $key. The allowlist may only remove the frozen R2 entry."
        }

        $entries.Add([pscustomobject]@{
                Key = $key
                Path = $relativePath
                Violation = $violation
                RemovalRound = $removalRound
            })
    }

    return [pscustomobject]@{
        Entries = @($entries)
        ByPath = $entries | Group-Object -Property Path -AsHashTable -AsString
    }
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
        [pscustomobject]@{ Category = 'product-code-literal'; Pattern = '(?i)["''](?:TS|TF|TL|T)["'']' },
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

try {
    if (-not (Test-Path -LiteralPath $script:RepositoryRoot -PathType Container)) {
        throw "Repository root is missing: $script:RepositoryRoot"
    }

    $allowlist = Read-LayeringAllowlist
    $marketBranchViolations = @(Get-MarketBranchViolations)
    if ($marketBranchViolations.Count -gt 0) {
        foreach ($violation in $marketBranchViolations) {
            Write-Error ("AC03 market branch at {0}:{1}: {2}" -f $violation.Path, $violation.Line, $violation.Text)
        }
        throw "Layering gate failed AC03 with $($marketBranchViolations.Count) violation(s)."
    }

    $domainRuleViolations = @(Get-DomainRuleViolations)
    $allowlistedViolations = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    $unexpectedRuleViolations = [System.Collections.Generic.List[object]]::new()
    foreach ($violation in $domainRuleViolations) {
        $matchingEntries = @($allowlist.ByPath[$violation.Path])
        $matchingEntry = $matchingEntries | Where-Object {
            $_.Violation -eq 'market-rule-values'
        } | Select-Object -First 1
        if ($null -eq $matchingEntry) {
            $unexpectedRuleViolations.Add($violation)
        }
        else {
            [void]$allowlistedViolations.Add($matchingEntry.Key)
        }
    }

    if ($unexpectedRuleViolations.Count -gt 0) {
        foreach ($violation in $unexpectedRuleViolations) {
            Write-Error ("AC01 domain rule value at {0}:{1} ({2}): {3}" -f $violation.Path, $violation.Line, $violation.Category, $violation.Text)
        }
        throw "Layering gate failed AC01 with $($unexpectedRuleViolations.Count) non-allowlisted violation(s)."
    }

    foreach ($entry in $allowlist.Entries) {
        if (-not $allowlistedViolations.Contains($entry.Key)) {
            throw "Stale layering allowlist entry has no matching current violation: $($entry.Key)"
        }
    }

    Write-Host ("Layering gate passed: AC03=0 market branches; AC01=$($domainRuleViolations.Count) domain rule value finding(s), allowlisted=$($allowlistedViolations.Count).")
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
