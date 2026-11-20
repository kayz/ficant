[CmdletBinding()]
param(
    [switch]$ListOnly,
    [switch]$Bp1Protected,
    [switch]$Bp1ProtectedFinal,
    [switch]$Bp1ProtectedSelfTest,
    [switch]$Bp1Docs,
    [switch]$RequireCrossClang
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$script:Bp1BaselineCommit = '8a91799767472af5254d23f6355ba8c729d9698e'
$script:Bp1ProtectedRoots = @(
    '.github',
    'cicd.yml',
    'deploy',
    'domain-packs',
    'tests/golden-cases/china-rates',
    'tests/oracle/china-rates'
)
$script:Bp1AllowedNewFiles = @(
    'tests/golden-cases/china-rates/bp1-futures-hedge-v2-inputs.json',
    'tests/golden-cases/china-rates/expected/bp1-futures-hedge-v2-expected.json',
    'tests/oracle/china-rates/bp1_futures_hedge_v2_decimal_oracle.py',
    'tests/oracle/china-rates/test_bp1_futures_hedge_v2_decimal_oracle.py',
    'tests/golden-cases/china-rates/bp1-policy-bank-pretax-inputs.json',
    'tests/golden-cases/china-rates/expected/bp1-policy-bank-pretax-expected.json',
    'tests/oracle/china-rates/bp1_policy_bank_pretax_decimal_oracle.py',
    'tests/oracle/china-rates/test_bp1_policy_bank_pretax_decimal_oracle.py'
)

function Invoke-Bp1GitRaw {
    param(
        [Parameter(Mandatory)]
        [string[]]$ArgumentList
    )

    $psi = [System.Diagnostics.ProcessStartInfo]::new('git')
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [System.Text.UTF8Encoding]::new($false)
    $psi.StandardErrorEncoding = [System.Text.UTF8Encoding]::new($false)
    foreach ($arg in $ArgumentList) {
        [void]$psi.ArgumentList.Add($arg)
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $psi
    try {
        if (-not $process.Start()) {
            throw "Failed to start git process: git $($ArgumentList -join ' ')"
        }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        $stdout = $stdoutTask.GetAwaiter().GetResult()
        $stderr = $stderrTask.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0) {
            throw "git exited with code $($process.ExitCode): git $($ArgumentList -join ' ')`n$($stderr.TrimEnd())"
        }
        return $stdout
    }
    finally {
        $process.Dispose()
    }
}

function Split-Bp1NulTokens {
    param(
        [Parameter(Mandatory)]
        [AllowEmptyString()]
        [string]$Raw,

        [Parameter(Mandatory)]
        [string]$Label
    )

    if ($Raw.Length -eq 0) {
        return @()
    }
    if ($Raw[$Raw.Length - 1] -ne [char]0) {
        throw "BP-1 protected check ($Label) received non-NUL-terminated git output."
    }
    return @($Raw.Substring(0, $Raw.Length - 1).Split([char]0))
}

function Test-Bp1RegularDiskFile {
    param(
        [Parameter(Mandatory)][string]$RepositoryRoot,
        [Parameter(Mandatory)][string]$RelativePath
    )

    $fullPath = Join-Path $RepositoryRoot ($RelativePath.Replace('/', [System.IO.Path]::DirectorySeparatorChar))
    if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
        return $false
    }
    $item = Get-Item -LiteralPath $fullPath -Force -ErrorAction Stop
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        return $false
    }
    if ($null -ne $item.LinkType) {
        return $false
    }
    return $true
}

function Test-Bp1RegularTreeMode {
    param(
        [Parameter(Mandatory)][string]$RepositoryRoot,
        [Parameter(Mandatory)][string]$Commit,
        [Parameter(Mandatory)][string]$RelativePath
    )

    $raw = Invoke-Bp1GitRaw -ArgumentList @('-C', $RepositoryRoot, 'ls-tree', '-z', $Commit, '--', $RelativePath)
    $tokens = @(Split-Bp1NulTokens -Raw $raw -Label "ls-tree $Commit $RelativePath")
    if ($tokens.Count -ne 1) {
        return $false
    }
    $entry = $tokens[0]
    $parts = $entry -split "`t", 2
    if ($parts.Count -ne 2 -or $parts[1] -cne $RelativePath) {
        return $false
    }
    return $parts[0] -cmatch '^(100644|100755) blob [0-9a-f]{40,}$'
}

function Test-Bp1RegularIndexMode {
    param(
        [Parameter(Mandatory)][string]$RepositoryRoot,
        [Parameter(Mandatory)][string]$RelativePath
    )

    $raw = Invoke-Bp1GitRaw -ArgumentList @('-C', $RepositoryRoot, 'ls-files', '--stage', '-z', '--', $RelativePath)
    $tokens = @(Split-Bp1NulTokens -Raw $raw -Label "ls-files --stage $RelativePath")
    if ($tokens.Count -ne 1) {
        return $false
    }
    $entry = $tokens[0]
    $parts = $entry -split "`t", 2
    if ($parts.Count -ne 2 -or $parts[1] -cne $RelativePath) {
        return $false
    }
    return $parts[0] -cmatch '^(100644|100755) [0-9a-f]{40,} 0$'
}

function Invoke-Bp1ProtectedCheck {
    param(
        [Parameter(Mandatory)][string]$RepositoryRoot,
        [Parameter(Mandatory)][string]$BaseCommit,
        [switch]$Final
    )

    Assert-FicantCommand 'git'
    [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $RepositoryRoot, 'rev-parse', '--verify', '--quiet', "$BaseCommit^{commit}"))

    $allowedSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($path in $script:Bp1AllowedNewFiles) {
        [void]$allowedSet.Add($path)
    }

    $baselineAllowlistRaw = Invoke-Bp1GitRaw -ArgumentList (@('-C', $RepositoryRoot, 'ls-tree', '-r', '-z', '--name-only', $BaseCommit, '--') + $script:Bp1AllowedNewFiles)
    $baselineAllowlistTokens = @(Split-Bp1NulTokens -Raw $baselineAllowlistRaw -Label 'baseline allowlist check')
    if ($baselineAllowlistTokens.Count -gt 0) {
        throw "Allowlisted BP-1 new file(s) already exist in baseline commit ${BaseCommit}: $($baselineAllowlistTokens -join ', ')"
    }

    $violations = [System.Collections.Generic.List[string]]::new()

    $diffLayers = @(
        [pscustomobject]@{
            Name = 'diff(BASE..HEAD)'
            Kind = 'head'
            Args = @('-C', $RepositoryRoot, 'diff', '--no-renames', '--name-status', '-z', $BaseCommit, 'HEAD', '--') + $script:Bp1ProtectedRoots
        },
        [pscustomobject]@{
            Name = 'diff(--cached BASE)'
            Kind = 'index'
            Args = @('-C', $RepositoryRoot, 'diff', '--cached', '--no-renames', '--name-status', '-z', $BaseCommit, '--') + $script:Bp1ProtectedRoots
        },
        [pscustomobject]@{
            Name = 'diff(BASE..worktree)'
            Kind = 'worktree'
            Args = @('-C', $RepositoryRoot, 'diff', '--no-renames', '--name-status', '-z', $BaseCommit, '--') + $script:Bp1ProtectedRoots
        }
    )

    foreach ($layer in $diffLayers) {
        $raw = Invoke-Bp1GitRaw -ArgumentList $layer.Args
        $tokens = @(Split-Bp1NulTokens -Raw $raw -Label $layer.Name)
        $index = 0
        while ($index -lt $tokens.Count) {
            $status = $tokens[$index]
            if ($status.StartsWith('R', [System.StringComparison]::Ordinal) -or
                $status.StartsWith('C', [System.StringComparison]::Ordinal)) {
                if ($index + 2 -ge $tokens.Count) {
                    throw "Malformed rename/copy record in $($layer.Name)."
                }
                $oldPath = $tokens[$index + 1]
                $newPath = $tokens[$index + 2]
                $violations.Add("$($layer.Name): forbidden status '$status' ($oldPath -> $newPath)")
                $index += 3
                continue
            }
            if ($index + 1 -ge $tokens.Count) {
                throw "Malformed name-status record in $($layer.Name) at token $index."
            }
            $path = $tokens[$index + 1]
            $index += 2

            if ($status -cne 'A') {
                $violations.Add("$($layer.Name): forbidden change '$status' on '$path'")
                continue
            }
            if (-not $allowedSet.Contains($path)) {
                $violations.Add("$($layer.Name): non-allowlisted addition '$path'")
                continue
            }
            $isRegular = switch ($layer.Kind) {
                'head' { Test-Bp1RegularTreeMode -RepositoryRoot $RepositoryRoot -Commit 'HEAD' -RelativePath $path }
                'index' { Test-Bp1RegularIndexMode -RepositoryRoot $RepositoryRoot -RelativePath $path }
                'worktree' { Test-Bp1RegularDiskFile -RepositoryRoot $RepositoryRoot -RelativePath $path }
            }
            if (-not $isRegular) {
                $violations.Add("$($layer.Name): allowlisted addition is not a regular file: '$path'")
            }
        }
    }

    $statusRaw = Invoke-Bp1GitRaw -ArgumentList (@('-C', $RepositoryRoot, '--no-optional-locks', 'status', '--porcelain=v1', '-z', '-uall', '--') + $script:Bp1ProtectedRoots)
    $statusTokens = @(Split-Bp1NulTokens -Raw $statusRaw -Label 'status --porcelain=v1')
    $index = 0
    while ($index -lt $statusTokens.Count) {
        $entry = $statusTokens[$index]
        if ($entry.Length -lt 4 -or $entry[2] -ne ' ') {
            throw "Malformed porcelain status record at token ${index}: '$entry'"
        }
        $xy = $entry.Substring(0, 2)
        $x = $entry[0]
        $y = $entry[1]
        $path = $entry.Substring(3)

        if ($x -in @('R', 'C') -or $y -in @('R', 'C')) {
            $origPath = if ($index + 1 -lt $statusTokens.Count) { $statusTokens[$index + 1] } else { '<missing>' }
            $violations.Add("status: forbidden rename/copy '$xy' ('$origPath' -> '$path')")
            $index += 2
            continue
        }
        $index += 1

        if ($xy -in @('DD', 'AU', 'UD', 'UA', 'DU', 'AA', 'UU') -or $x -eq 'U' -or $y -eq 'U') {
            $violations.Add("status: conflict '$xy' on '$path'")
            continue
        }
        if (-not $allowedSet.Contains($path)) {
            $violations.Add("status: forbidden change '$xy' on '$path'")
            continue
        }
        if ($xy -notin @('??', 'A ', 'AM', ' M', 'M ', 'MM')) {
            $violations.Add("status: forbidden state '$xy' on allowlisted path '$path'")
            continue
        }
        if (-not (Test-Bp1RegularDiskFile -RepositoryRoot $RepositoryRoot -RelativePath $path)) {
            $violations.Add("status: allowlisted path is not a regular file on disk: '$path'")
            continue
        }
        if ($x -ne '?' -and -not (Test-Bp1RegularIndexMode -RepositoryRoot $RepositoryRoot -RelativePath $path)) {
            $violations.Add("status: allowlisted path is not a regular file in index: '$path'")
        }
    }

    if ($Final) {
        foreach ($allowedPath in $script:Bp1AllowedNewFiles) {
            if (-not (Test-Bp1RegularDiskFile -RepositoryRoot $RepositoryRoot -RelativePath $allowedPath)) {
                $violations.Add("final: required allowlisted file is missing or not a regular file: '$allowedPath'")
            }
        }
    }

    if ($violations.Count -gt 0) {
        $details = ($violations | ForEach-Object { "  - $_" }) -join [Environment]::NewLine
        throw "BP-1 protected roots check failed with $($violations.Count) violation(s):`n$details"
    }
}

function Write-Bp1TestFile {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string]$RelativePath,
        [Parameter(Mandatory)][string]$Content
    )

    $fullPath = Join-Path $Root ($RelativePath.Replace('/', [System.IO.Path]::DirectorySeparatorChar))
    $parent = Split-Path -Parent $fullPath
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    $utf8 = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($fullPath, $Content, $utf8)
}

function Assert-Bp1ProtectedExpectFailure {
    param(
        [Parameter(Mandatory)][string]$CaseName,
        [Parameter(Mandatory)][string]$RepositoryRoot,
        [Parameter(Mandatory)][string]$BaseCommit,
        [switch]$Final
    )

    $failed = $false
    try {
        Invoke-Bp1ProtectedCheck -RepositoryRoot $RepositoryRoot -BaseCommit $BaseCommit -Final:$Final
    }
    catch {
        $failed = $true
    }
    if (-not $failed) {
        throw "BP-1 protected self-test expected failure but passed: $CaseName"
    }
    Write-Host "  [PASS expected-reject] $CaseName"
}

function Invoke-Bp1ProtectedSelfTest {
    Assert-FicantCommand 'git'
    $tempBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    $runId = [Guid]::NewGuid().ToString('N')
    $tempRepo = [System.IO.Path]::GetFullPath((Join-Path $tempBase "ficant-bp1-protected-selftest-$runId"))
    if (-not $tempRepo.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to create self-test repository outside temporary directory: $tempRepo"
    }

    New-Item -ItemType Directory -Path $tempRepo | Out-Null
    try {
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'init', '-q'))
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'config', 'user.name', 'ficant-selftest'))
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'config', 'user.email', 'selftest@ficant.local'))
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'config', 'core.autocrlf', 'false'))

        $baselineFixtures = @{
            '.github/workflows/ci.yml' = "name: ci`n"
            'cicd.yml' = "version: 1`n"
            'deploy/README.md' = "# deploy`n"
            'domain-packs/cgb-futures/cgb-futures-v2.json' = "{}`n"
            'tests/golden-cases/china-rates/phase2d-futures-hedge-inputs.json' = "{}`n"
            'tests/oracle/china-rates/phase2b_quantlib_oracle.py' = "# frozen oracle`n"
            'tests/oracle/china-rates/test_oracle_hardening.py' = "# frozen hardening`n"
        }
        foreach ($entry in $baselineFixtures.GetEnumerator()) {
            Write-Bp1TestFile -Root $tempRepo -RelativePath $entry.Key -Content $entry.Value
        }
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'add', '.'))
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'commit', '-q', '-m', 'baseline'))
        $testBase = (Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'rev-parse', 'HEAD')).Trim()

        Write-Host 'Running BP-1 protected baseline self-test in isolated temporary git repository...'

        Invoke-Bp1ProtectedCheck -RepositoryRoot $tempRepo -BaseCommit $testBase
        Write-Host '  [PASS] clean baseline (intermediate mode)'

        Assert-Bp1ProtectedExpectFailure -CaseName 'clean baseline missing 8 files (final mode)' -RepositoryRoot $tempRepo -BaseCommit $testBase -Final

        # Negative 1: modify existing baseline file phase2b_quantlib_oracle.py
        Write-Bp1TestFile -Root $tempRepo -RelativePath 'tests/oracle/china-rates/phase2b_quantlib_oracle.py' -Content "# tampered`n"
        Assert-Bp1ProtectedExpectFailure -CaseName 'modified phase2b_quantlib_oracle.py' -RepositoryRoot $tempRepo -BaseCommit $testBase
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'checkout', '--', 'tests/oracle/china-rates/phase2b_quantlib_oracle.py'))

        # Negative 2: delete existing baseline file test_oracle_hardening.py
        Remove-Item -LiteralPath (Join-Path $tempRepo 'tests\oracle\china-rates\test_oracle_hardening.py') -Force
        Assert-Bp1ProtectedExpectFailure -CaseName 'deleted test_oracle_hardening.py' -RepositoryRoot $tempRepo -BaseCommit $testBase
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'checkout', '--', 'tests/oracle/china-rates/test_oracle_hardening.py'))

        # Negative 3: add untracked non-allowlisted file under protected root
        $rogueRelative = 'tests/oracle/china-rates/untracked_rogue.py'
        Write-Bp1TestFile -Root $tempRepo -RelativePath $rogueRelative -Content "# rogue`n"
        Assert-Bp1ProtectedExpectFailure -CaseName 'untracked non-allowlisted file' -RepositoryRoot $tempRepo -BaseCommit $testBase

        # Negative 4: stage non-allowlisted file
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'add', $rogueRelative))
        Assert-Bp1ProtectedExpectFailure -CaseName 'staged non-allowlisted file' -RepositoryRoot $tempRepo -BaseCommit $testBase
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'reset', '-q', 'HEAD', '--', $rogueRelative))
        Remove-Item -LiteralPath (Join-Path $tempRepo ($rogueRelative.Replace('/', [System.IO.Path]::DirectorySeparatorChar))) -Force

        # Intermediate incremental addition: 1 of 8 allowlisted files
        $firstAllowed = $script:Bp1AllowedNewFiles[0]
        Write-Bp1TestFile -Root $tempRepo -RelativePath $firstAllowed -Content "{}`n"
        Invoke-Bp1ProtectedCheck -RepositoryRoot $tempRepo -BaseCommit $testBase
        Write-Host '  [PASS] 1/8 allowlisted untracked file (intermediate mode)'
        Assert-Bp1ProtectedExpectFailure -CaseName '1/8 allowlisted file incomplete (final mode)' -RepositoryRoot $tempRepo -BaseCommit $testBase -Final

        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'add', $firstAllowed))
        Invoke-Bp1ProtectedCheck -RepositoryRoot $tempRepo -BaseCommit $testBase
        Write-Host '  [PASS] 1/8 allowlisted staged file (intermediate mode)'

        # All 8 allowlisted files committed
        foreach ($allowedPath in $script:Bp1AllowedNewFiles) {
            Write-Bp1TestFile -Root $tempRepo -RelativePath $allowedPath -Content "# bp1 allowlisted`n"
        }
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'add', '.'))
        [void](Invoke-Bp1GitRaw -ArgumentList @('-C', $tempRepo, 'commit', '-q', '-m', 'add all 8 allowlisted files'))
        Invoke-Bp1ProtectedCheck -RepositoryRoot $tempRepo -BaseCommit $testBase
        Invoke-Bp1ProtectedCheck -RepositoryRoot $tempRepo -BaseCommit $testBase -Final
        Write-Host '  [PASS] 8/8 allowlisted files committed (intermediate + final mode)'

        # Deleting one of the 8 allowlisted files must fail
        Remove-Item -LiteralPath (Join-Path $tempRepo ($firstAllowed.Replace('/', [System.IO.Path]::DirectorySeparatorChar))) -Force
        Assert-Bp1ProtectedExpectFailure -CaseName 'deleted allowlisted file (final mode)' -RepositoryRoot $tempRepo -BaseCommit $testBase -Final
        Assert-Bp1ProtectedExpectFailure -CaseName 'deleted allowlisted file (intermediate mode)' -RepositoryRoot $tempRepo -BaseCommit $testBase

        Write-Host 'BP-1 protected baseline self-test passed.'
    }
    finally {
        if (-not [string]::IsNullOrWhiteSpace($tempRepo) -and
            (Test-Path -LiteralPath $tempRepo -PathType Container)) {
            $resolvedRepo = (Resolve-Path -LiteralPath $tempRepo).Path
            if (-not $resolvedRepo.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase) -or
                (Split-Path -Leaf $resolvedRepo) -notmatch '^ficant-bp1-protected-selftest-[0-9a-f]{32}$') {
                throw "Refusing to remove unexpected temporary directory: $resolvedRepo"
            }
            Remove-Item -LiteralPath $resolvedRepo -Recurse -Force
        }
    }
}

function Get-Bp1VisibleMarkdownText {
    param(
        [Parameter(Mandatory)]
        [AllowEmptyString()]
        [string]$Content
    )

    $withoutInlineTargets = [regex]::Replace($Content, '!?\[([^\]]*)\]\([^)]*\)', '$1')
    $withoutRefTargets = [regex]::Replace($withoutInlineTargets, '(?m)^(\s*\[[^\]]+\]:)\s*\S.*$', '$1')
    return $withoutRefTargets
}

function Find-Bp1ForbiddenTermHits {
    param(
        [Parameter(Mandatory)]
        [AllowEmptyString()]
        [string]$VisibleText
    )

    $hits = [System.Collections.Generic.List[string]]::new()
    $lines = $VisibleText -split "`r?`n"
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $lineNum = $i + 1
        $line = $lines[$i]
        if ($line -cmatch '\bIBOR\b') {
            $hits.Add("line ${lineNum}: forbidden standalone uppercase token 'IBOR'")
        }
        if ($line -imatch '\bInvestment\s+Book\s+of\s+Record\b') {
            $hits.Add("line ${lineNum}: forbidden term 'Investment Book of Record'")
        }
        if ($line -cmatch '投资记账簿') {
            $hits.Add("line ${lineNum}: forbidden term '投资记账簿'")
        }
    }
    if ($hits.Count -eq 0 -and ($VisibleText -imatch '\bInvestment\s+Book\s+of\s+Record\b')) {
        $hits.Add("multi-line: forbidden term 'Investment Book of Record'")
    }
    return @($hits.ToArray())
}

function Get-Bp1MarkdownLocalLinks {
    param(
        [Parameter(Mandatory)]
        [AllowEmptyString()]
        [string]$Content
    )

    $links = [System.Collections.Generic.List[pscustomobject]]::new()
    $lines = $Content -split "`r?`n"
    $inFence = $false
    $fenceChar = ''
    $fenceLength = 0

    for ($i = 0; $i -lt $lines.Count; $i++) {
        $lineNum = $i + 1
        $line = $lines[$i]

        if (-not $inFence) {
            if ($line -match '^\s{0,3}(`{3,}|~{3,})') {
                $marker = $Matches[1]
                $inFence = $true
                $fenceChar = $marker[0]
                $fenceLength = $marker.Length
                continue
            }
        }
        else {
            if ($line -match "^\s{0,3}$([regex]::Escape($fenceChar)){$fenceLength,}\s*$") {
                $inFence = $false
                $fenceChar = ''
                $fenceLength = 0
            }
            continue
        }

        $inlineMatches = [regex]::Matches($line, '!?\[[^\]]*\]\(([^)]+)\)')
        foreach ($match in $inlineMatches) {
            $raw = $match.Groups[1].Value.Trim()
            if ($raw.StartsWith('<') -and $raw.Contains('>')) {
                $closeIdx = $raw.IndexOf('>')
                $target = $raw.Substring(1, $closeIdx - 1).Trim()
            }
            else {
                $target = ($raw -split '\s+', 2)[0]
            }
            if (-not [string]::IsNullOrWhiteSpace($target)) {
                $links.Add([pscustomobject]@{
                    Line = $lineNum
                    RawTarget = $target
                })
            }
        }

        if ($line -match '^\s*\[[^\]]+\]:\s*(\S+)') {
            $refTarget = $Matches[1].Trim()
            if ($refTarget.StartsWith('<') -and $refTarget.EndsWith('>')) {
                $refTarget = $refTarget.Substring(1, $refTarget.Length - 2).Trim()
            }
            if (-not [string]::IsNullOrWhiteSpace($refTarget)) {
                $links.Add([pscustomobject]@{
                    Line = $lineNum
                    RawTarget = $refTarget
                })
            }
        }
    }

    return @($links.ToArray())
}

function Invoke-Bp1DocsCheck {
    param(
        [Parameter(Mandatory)][string]$RepositoryRoot
    )

    Assert-FicantCommand 'git'
    $utf8 = [System.Text.UTF8Encoding]::new($false)
    $repoRootFull = [System.IO.Path]::GetFullPath($RepositoryRoot)
    $repoRootWithSep = $repoRootFull.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    $pathComparer = if ($IsWindows) { [System.StringComparison]::OrdinalIgnoreCase } else { [System.StringComparison]::Ordinal }

    # Self-verify matcher boundaries: allow Shibor3M and -ibor.md link target, reject display text IBOR / legacy terms.
    if (@(Find-Bp1ForbiddenTermHits -VisibleText (Get-Bp1VisibleMarkdownText -Content 'Shibor3M [ADR-0030](docs/architecture/adr/0030-core-reset-and-trading-desk-ibor.md)')).Count -ne 0) {
        throw 'BP-1 docs check matcher self-assertion failed: Shibor3M or -ibor.md link destination falsely triggered.'
    }
    if (@(Find-Bp1ForbiddenTermHits -VisibleText (Get-Bp1VisibleMarkdownText -Content '[实时 IBOR](docs/architecture/adr/0030-core-reset-and-trading-desk-ibor.md)')).Count -eq 0) {
        throw 'BP-1 docs check matcher self-assertion failed: display text IBOR was not detected.'
    }
    if (@(Find-Bp1ForbiddenTermHits -VisibleText 'Investment Book of Record').Count -eq 0) {
        throw 'BP-1 docs check matcher self-assertion failed: Investment Book of Record was not detected.'
    }
    if (@(Find-Bp1ForbiddenTermHits -VisibleText '投资记账簿').Count -eq 0) {
        throw 'BP-1 docs check matcher self-assertion failed: 投资记账簿 was not detected.'
    }

    $violations = [System.Collections.Generic.List[string]]::new()
    $adrRelPath = 'docs/architecture/adr/0030-core-reset-and-trading-desk-ibor.md'
    $adrFullPath = [System.IO.Path]::GetFullPath((Join-Path $repoRootFull ($adrRelPath.Replace('/', [System.IO.Path]::DirectorySeparatorChar))))
    $positioningDocs = @(
        'AGENTS.md',
        'README.md',
        $adrRelPath
    )

    foreach ($docRelPath in $positioningDocs) {
        if (-not (Test-Bp1RegularDiskFile -RepositoryRoot $repoRootFull -RelativePath $docRelPath)) {
            $violations.Add("${docRelPath}: required positioning document is missing or not a regular file")
            continue
        }
        $docFullPath = [System.IO.Path]::GetFullPath((Join-Path $repoRootFull ($docRelPath.Replace('/', [System.IO.Path]::DirectorySeparatorChar))))
        $content = [System.IO.File]::ReadAllText($docFullPath, $utf8)
        $visible = Get-Bp1VisibleMarkdownText -Content $content

        $hits = @(Find-Bp1ForbiddenTermHits -VisibleText $visible)
        foreach ($hit in $hits) {
            $violations.Add("${docRelPath}: $hit")
        }

        if ($visible -notmatch '盘中实时簿记' -or $visible -notmatch 'Desk Blotter') {
            $violations.Add("${docRelPath}: missing required new positioning terminology '盘中实时簿记' and 'Desk Blotter'")
        }
        if ($visible -notmatch '盘中实时簿记（Desk Blotter）') {
            $violations.Add("${docRelPath}: missing canonical phrase '盘中实时簿记（Desk Blotter）'")
        }

        if ($docRelPath -ceq $adrRelPath) {
            $visibleLines = $visible -split "`r?`n"
            if ($visibleLines.Count -eq 0) {
                $violations.Add("${docRelPath}: empty document")
            }
            else {
                $titleLine = $visibleLines[0]
                if ($titleLine -notmatch '^#\s*ADR-0030' -or $titleLine -notmatch '盘中实时簿记' -or $titleLine -notmatch 'Desk Blotter') {
                    $violations.Add("${docRelPath}: line 1 title must switch to '盘中实时簿记' / 'Desk Blotter'")
                }
                $decisionMatch = [regex]::Match($visible, '(?ms)^##\s+决策\s*$(.*?)(?=^##\s+|\z)')
                if (-not $decisionMatch.Success -or $decisionMatch.Groups[1].Value -notmatch '盘中实时簿记（Desk Blotter）') {
                    $violations.Add("${docRelPath}: '## 决策' positioning section must switch to '盘中实时簿记（Desk Blotter）'")
                }
                $consequenceMatch = [regex]::Match($visible, '(?ms)^##\s+后果\s*$(.*?)(?=^##\s+|\z)')
                if (-not $consequenceMatch.Success -or $consequenceMatch.Groups[1].Value -notmatch '盘中实时簿记' -or $consequenceMatch.Groups[1].Value -notmatch 'Desk Blotter') {
                    $violations.Add("${docRelPath}: '## 后果' section must switch to '盘中实时簿记' / 'Desk Blotter'")
                }
            }
        }
    }

    $trackedMdRaw = Invoke-Bp1GitRaw -ArgumentList @('-C', $repoRootFull, 'ls-files', '-z', '--', '*.md')
    $trackedMdFiles = @(Split-Bp1NulTokens -Raw $trackedMdRaw -Label 'ls-files *.md')
    if ($trackedMdFiles.Count -eq 0) {
        $violations.Add('repository has no tracked Markdown files')
    }

    $localLinksChecked = 0
    $brokenLinkCount = 0
    $agentsAdrIncomingLinks = 0

    foreach ($mdRelPath in $trackedMdFiles) {
        $mdFullPath = [System.IO.Path]::GetFullPath((Join-Path $repoRootFull ($mdRelPath.Replace('/', [System.IO.Path]::DirectorySeparatorChar))))
        if (-not (Test-Path -LiteralPath $mdFullPath -PathType Leaf)) {
            $violations.Add("${mdRelPath}: tracked Markdown file is missing on disk")
            continue
        }
        $mdDir = Split-Path -Parent $mdFullPath
        $mdContent = [System.IO.File]::ReadAllText($mdFullPath, $utf8)
        $links = @(Get-Bp1MarkdownLocalLinks -Content $mdContent)
        foreach ($link in $links) {
            $rawTarget = $link.RawTarget
            if ($rawTarget.StartsWith('#') -or $rawTarget -imatch '^(https?|mailto|tel|ftp|data):') {
                continue
            }
            $pathPart = ($rawTarget -split '[#?]', 2)[0]
            if ([string]::IsNullOrWhiteSpace($pathPart)) {
                continue
            }
            $decodedPath = [System.Uri]::UnescapeDataString($pathPart)
            $normalizedRel = $decodedPath.Replace('/', [System.IO.Path]::DirectorySeparatorChar)
            $resolvedTarget = if ($decodedPath.StartsWith('/')) {
                [System.IO.Path]::GetFullPath((Join-Path $repoRootFull $normalizedRel.TrimStart([System.IO.Path]::DirectorySeparatorChar)))
            }
            else {
                [System.IO.Path]::GetFullPath((Join-Path $mdDir $normalizedRel))
            }

            $localLinksChecked += 1
            $insideRepo = $resolvedTarget.Equals($repoRootFull, $pathComparer) -or $resolvedTarget.StartsWith($repoRootWithSep, $pathComparer)
            if (-not $insideRepo -or -not (Test-Path -LiteralPath $resolvedTarget)) {
                $brokenLinkCount += 1
                $violations.Add("${mdRelPath}:$($link.Line): broken local relative link '$rawTarget' (resolved: '$resolvedTarget')")
                continue
            }

            if ($mdRelPath -ceq 'AGENTS.md' -and $resolvedTarget.Equals($adrFullPath, $pathComparer)) {
                $agentsAdrIncomingLinks += 1
            }
        }
    }

    if ($agentsAdrIncomingLinks -lt 1) {
        $violations.Add("AGENTS.md: missing valid incoming link to '$adrRelPath'")
    }

    if ($violations.Count -gt 0) {
        $details = ($violations | ForEach-Object { "  - $_" }) -join [Environment]::NewLine
        throw "BP-1 docs check failed with $($violations.Count) violation(s) (broken_links=$brokenLinkCount):`n$details"
    }

    Write-Host "BP-1 docs check passed (docs=$($positioningDocs.Count), agents_adr_incoming_links=$agentsAdrIncomingLinks, tracked_md_files=$($trackedMdFiles.Count), local_links_checked=$localLinksChecked, broken_links=0)."
}

$visualStudioClang = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\clang++.exe'
$cppCompiler = if (-not [string]::IsNullOrWhiteSpace($env:FICANT_CXX)) {
    $env:FICANT_CXX
}
elseif (Test-Path -LiteralPath $visualStudioClang -PathType Leaf) {
    $visualStudioClang
}
else {
    'clang++'
}
$cppBuildDirectory = Join-Path $script:FicantRoot 'build\local-cpp-vs-llvm-19'
$cmakeConfigureArgs = @(
    '-S', 'cpp/fixed-income-kernel',
    '-B', $cppBuildDirectory,
    '-G', 'Ninja',
    "-DCMAKE_CXX_COMPILER=$cppCompiler",
    '-DCMAKE_BUILD_TYPE=Release'
)
if ($IsWindows -and [System.IO.Path]::IsPathRooted($cppCompiler)) {
    $llvmRc = Join-Path (Split-Path -Parent $cppCompiler) 'llvm-rc.exe'
    if (Test-Path -LiteralPath $llvmRc -PathType Leaf) {
        $cmakeConfigureArgs += "-DCMAKE_RC_COMPILER=$($llvmRc.Replace('\', '/'))"
    }
}

$effectiveRequireCrossClang = $RequireCrossClang -or ($env:FICANT_REQUIRE_CROSS_CLANG -eq '1')
$crossClangArgs = @('-NoProfile', '-NonInteractive', '-File', 'scripts/check-cross-clang.ps1')
if ($effectiveRequireCrossClang) {
    $crossClangArgs += '-RequireCrossClang'
}

$steps = @(
    New-FicantCheckStep -Name 'Layering gate' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/check-layering.ps1')
    New-FicantCheckStep -Name 'Layering gate fixture tests' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/test-layering-check.ps1')
    New-FicantCheckStep -Name 'Buf format check' -FilePath 'buf' -ArgumentList @('format', '--diff', '--exit-code', 'interface')
    New-FicantCheckStep -Name 'Buf lint' -FilePath 'buf' -ArgumentList @('lint', 'interface')
    New-FicantCheckStep -Name 'CGB futures RulePack payload drift' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/generate-cgb-futures-pack.ps1', '-Check')
    New-FicantCheckStep -Name 'CGB interest-tax RulePack payload drift' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/generate-cgb-interest-tax-pack.ps1', '-Check')
    New-FicantCheckStep -Name 'Rust formatting' -FilePath 'cargo' -ArgumentList @('fmt', '--all', '--', '--check')
    New-FicantCheckStep -Name 'Rust strict Clippy' -FilePath 'cargo' -ArgumentList @('clippy', '--workspace', '--all-targets', '--locked', '--exclude', 'ficant-contracts', '--no-deps', '--', '-D', 'warnings')
    New-FicantCheckStep -Name 'Rust workspace build' -FilePath 'cargo' -ArgumentList @('build', '--workspace', '--all-targets', '--locked')
    New-FicantCheckStep -Name 'Rust workspace tests' -FilePath 'cargo' -ArgumentList @('test', '--workspace', '--locked')
    New-FicantCheckStep -Name 'C++ configure' -FilePath 'cmake' -ArgumentList $cmakeConfigureArgs
    New-FicantCheckStep -Name 'C++ build' -FilePath 'cmake' -ArgumentList @('--build', $cppBuildDirectory, '--parallel')
    New-FicantCheckStep -Name 'C++ tests' -FilePath 'ctest' -ArgumentList @('--test-dir', $cppBuildDirectory, '--output-on-failure')
    New-FicantCheckStep -Name 'Cross-Clang comparator fixture tests' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/test-cross-clang-check.ps1')
    New-FicantCheckStep -Name 'Cross-Clang raw numeric identity' -FilePath 'pwsh' -ArgumentList $crossClangArgs
    New-FicantCheckStep -Name 'Phase 2B independent Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_phase2b_manual_oracle.py', '-q')
    New-FicantCheckStep -Name 'Phase 2C independent Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_phase2c_manual_oracle.py', '-q')
    New-FicantCheckStep -Name 'Phase 2D independent Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_phase2d_manual_oracle.py', '-q')
    New-FicantCheckStep -Name 'Portfolio KRD independent Decimal Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_r5d_portfolio_krd_decimal_oracle.py', '-q')
    New-FicantCheckStep -Name 'Tax-adjusted independent Decimal Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_r5e_tax_adjusted_decimal_oracle.py', '-q')
    New-FicantCheckStep -Name 'BP-1 CGB futures v2 independent Decimal Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_bp1_futures_hedge_v2_decimal_oracle.py', '-q')
    New-FicantCheckStep -Name 'BP-1 policy bank PRE_TAX independent Decimal Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_bp1_policy_bank_pretax_decimal_oracle.py', '-q')
    New-FicantCheckStep -Name 'Python contracts and live SDK' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', '-q', 'python/tests')
)

try {
    $selectedModes = @(@($ListOnly.IsPresent, $Bp1Protected.IsPresent, $Bp1ProtectedFinal.IsPresent, $Bp1ProtectedSelfTest.IsPresent, $Bp1Docs.IsPresent) | Where-Object { $_ })
    if ($selectedModes.Count -gt 1) {
        throw 'Specify at most one of -ListOnly, -Bp1Protected, -Bp1ProtectedFinal, -Bp1ProtectedSelfTest, or -Bp1Docs.'
    }

    if ($Bp1ProtectedSelfTest) {
        Invoke-Bp1ProtectedSelfTest
        exit 0
    }
    if ($Bp1Protected) {
        Invoke-Bp1ProtectedCheck -RepositoryRoot $script:FicantRoot -BaseCommit $script:Bp1BaselineCommit
        Write-Host "BP-1 protected roots check passed (intermediate mode, base=$script:Bp1BaselineCommit)."
        exit 0
    }
    if ($Bp1ProtectedFinal) {
        Invoke-Bp1ProtectedCheck -RepositoryRoot $script:FicantRoot -BaseCommit $script:Bp1BaselineCommit -Final
        Write-Host "BP-1 protected roots check passed (final mode, base=$script:Bp1BaselineCommit, 8/8 allowlisted files present)."
        exit 0
    }
    if ($Bp1Docs) {
        Invoke-Bp1DocsCheck -RepositoryRoot $script:FicantRoot
        exit 0
    }
    if ($ListOnly) {
        Show-FicantCheckPlan -Steps $steps
        exit 0
    }

    Assert-FicantRustCapability
    foreach ($command in @('pwsh', 'cmake', 'ctest', 'ninja', 'uv', 'buf')) {
        Assert-FicantCommand $command
    }
    [void](Assert-FicantMinimumVersion -Name 'clang++' -FilePath $cppCompiler -ArgumentList @('--version') -Minimum '18.0.0' -Pinned '19.1.5')
    [void](Assert-FicantMinimumVersion -Name 'uv' -ArgumentList @('--version') -Minimum '0.7.0' -Pinned '0.7.13')
    [void](Assert-FicantMinimumVersion -Name 'buf' -ArgumentList @('--version') -Minimum '1.56.0' -Pinned '1.56.0')

    Invoke-FicantCheckPlan -Steps $steps
    Write-Host ''
    Write-Host 'FICANT full local checks passed.'
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
