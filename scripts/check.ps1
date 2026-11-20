[CmdletBinding()]
param(
    [switch]$ListOnly,
    [switch]$Bp1Protected,
    [switch]$Bp1ProtectedFinal,
    [switch]$Bp1ProtectedSelfTest,
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
    New-FicantCheckStep -Name 'Python contracts and live SDK' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', '-q', 'python/tests')
)

try {
    $selectedModes = @(@($ListOnly.IsPresent, $Bp1Protected.IsPresent, $Bp1ProtectedFinal.IsPresent, $Bp1ProtectedSelfTest.IsPresent) | Where-Object { $_ })
    if ($selectedModes.Count -gt 1) {
        throw 'Specify at most one of -ListOnly, -Bp1Protected, -Bp1ProtectedFinal, or -Bp1ProtectedSelfTest.'
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
