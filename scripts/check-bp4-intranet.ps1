[CmdletBinding()]
param(
    [switch]$ListOnly,
    [switch]$SkipBrowserE2E
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$script:Bp4BaselineCommit = '6751cb352e054af623a7e2df3d363117ee0026af'
$script:DockerfileSubsetEntries = @(
    'Cargo.toml',
    'Cargo.lock',
    'rust-toolchain.toml',
    'binaries',
    'crates',
    'cpp',
    'domain-packs',
    'interface'
)
$script:Bp4FrozenRoots = @(
    '.github',
    'cicd.yml',
    'deploy',
    'domain-packs',
    'tests/golden-cases',
    'tests/oracle',
    'cpp',
    'crates/ficant-kernel-sys',
    'crates/ficant-domain',
    'crates/ficant-application',
    'interface',
    'docs/delivery/test-environment.md',
    'Cargo.toml',
    'python/uv.lock'
)

function Invoke-GitRaw {
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

function Split-NulTokens {
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
        throw "BP-4 governance check ($Label) received non-NUL-terminated git output."
    }
    return @($Raw.Substring(0, $Raw.Length - 1).Split([char]0))
}

function Get-GitSha40 {
    param(
        [Parameter(Mandatory)]
        [string[]]$RevParseArgs,

        [Parameter(Mandatory)]
        [string]$Label
    )

    $raw = Invoke-GitRaw -ArgumentList (@('-C', $script:FicantRoot, 'rev-parse') + $RevParseArgs)
    $sha = $raw.Trim()
    if ($sha -cnotmatch '^[0-9a-f]{40}$') {
        throw "Resolved Git $Label is not a 40-character lowercase SHA: '$sha'"
    }
    return $sha
}

function Get-FreeLoopbackPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try {
        return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    }
    finally {
        $listener.Stop()
    }
}

function Copy-FilteredDirectory {
    param(
        [Parameter(Mandatory)]
        [string]$SourceDir,

        [Parameter(Mandatory)]
        [string]$DestinationDir,

        [string[]]$ExcludeDirNameRegexes = @()
    )

    [void](New-Item -ItemType Directory -Path $DestinationDir -Force)
    foreach ($entry in @(Get-ChildItem -LiteralPath $SourceDir -Force -ErrorAction Stop)) {
        if (($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or $null -ne $entry.LinkType) {
            throw "Unexpected symlink or reparse point in Dockerfile source subset: $($entry.FullName)"
        }
        $targetPath = Join-Path $DestinationDir $entry.Name
        if ($entry.PSIsContainer) {
            $excluded = $false
            foreach ($regex in $ExcludeDirNameRegexes) {
                if ($entry.Name -imatch $regex) {
                    $excluded = $true
                    break
                }
            }
            if ($excluded) {
                continue
            }
            Copy-FilteredDirectory -SourceDir $entry.FullName -DestinationDir $targetPath -ExcludeDirNameRegexes $ExcludeDirNameRegexes
        }
        else {
            [System.IO.File]::Copy($entry.FullName, $targetPath, $false)
        }
    }
}

function Remove-TempSubtreeSafely {
    param(
        [Parameter(Mandatory)]
        [string]$TempRoot,

        [Parameter(Mandatory)]
        [string]$TargetDir,

        [Parameter(Mandatory)]
        [string]$ExpectedLeafRegex
    )

    if ([string]::IsNullOrWhiteSpace($TargetDir) -or -not (Test-Path -LiteralPath $TargetDir -PathType Container)) {
        return
    }
    $resolved = (Resolve-Path -LiteralPath $TargetDir).Path
    if (-not $resolved.StartsWith($TempRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
        (Split-Path -Leaf $resolved) -notmatch $ExpectedLeafRegex) {
        throw "Refusing to remove unexpected directory outside temp scope: $resolved"
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}

function Stop-ServerProcessGracefully {
    param(
        [Parameter(Mandatory)]
        [System.Diagnostics.Process]$Process,

        [int]$TimeoutMs = 10000
    )

    if ($Process.HasExited) {
        return $Process.ExitCode
    }
    try {
        $Process.StandardInput.WriteLine('SHUTDOWN')
        $Process.StandardInput.Flush()
        $Process.StandardInput.Close()
    }
    catch {
        # Fall through to bounded exit wait / forced kill.
    }

    if ($Process.WaitForExit($TimeoutMs)) {
        return $Process.ExitCode
    }
    try {
        $Process.Kill($true)
        [void]$Process.WaitForExit(3000)
    }
    catch {
        # Best effort termination.
    }
    throw "Server process failed to exit cleanly within ${TimeoutMs}ms after SHUTDOWN."
}

function Invoke-Phase1DockerfileSubsetProof {
    param(
        [Parameter(Mandatory)]
        [string]$TempRoot,

        [Parameter(Mandatory)]
        [string]$CommitSha,

        [Parameter(Mandatory)]
        [string]$TreeSha
    )

    Write-Host ''
    Write-Host '==> Phase 1: Exact 8-path RustService.Dockerfile subset release build & disabled-UI boot proof (F-14, AC-001)'

    $runId = [Guid]::NewGuid().ToString('N')
    $phase1Root = [System.IO.Path]::GetFullPath((Join-Path $TempRoot "ficant-bp4-phase1-$runId"))
    $phase1Src = Join-Path $phase1Root 'src'
    $phase1Target = Join-Path $phase1Root 'target'
    $phase1RunDir = Join-Path $phase1Root 'run'

    [void](New-Item -ItemType Directory -Path $phase1Src -Force)
    [void](New-Item -ItemType Directory -Path $phase1Target -Force)
    [void](New-Item -ItemType Directory -Path $phase1RunDir -Force)

    $proc = $null
    try {
        foreach ($entryName in $script:DockerfileSubsetEntries) {
            $srcPath = Join-Path $script:FicantRoot $entryName
            $dstPath = Join-Path $phase1Src $entryName
            if (-not (Test-Path -LiteralPath $srcPath)) {
                throw "Required Dockerfile subset entry missing: $srcPath"
            }
            if (Test-Path -LiteralPath $srcPath -PathType Container) {
                $excludes = if ($entryName -ceq 'cpp') {
                    @('^build', '^\.git$', '^target$', '^__pycache__$')
                }
                else {
                    @('^\.git$', '^target$', '^node_modules$', '^__pycache__$')
                }
                Copy-FilteredDirectory -SourceDir $srcPath -DestinationDir $dstPath -ExcludeDirNameRegexes $excludes
            }
            else {
                [System.IO.File]::Copy($srcPath, $dstPath, $false)
            }
        }

        $actualTopEntries = @(Get-ChildItem -LiteralPath $phase1Src -Force | Select-Object -ExpandProperty Name | Sort-Object)
        $expectedTopEntries = @($script:DockerfileSubsetEntries | Sort-Object)
        if (($actualTopEntries -join '|') -cne ($expectedTopEntries -join '|')) {
            throw "Phase 1 source subset mismatch! Actual=[$($actualTopEntries -join ', ')] Expected=[$($expectedTopEntries -join ', ')]"
        }
        foreach ($forbidden in @('.git', 'apps', 'python', 'scripts', 'docs', 'deploy')) {
            if (Test-Path -LiteralPath (Join-Path $phase1Src $forbidden)) {
                throw "Phase 1 isolated subset unexpectedly contains forbidden entry: $forbidden"
            }
        }

        $buildPsi = [System.Diagnostics.ProcessStartInfo]::new('cargo')
        $buildPsi.WorkingDirectory = $phase1Src
        $buildPsi.UseShellExecute = $false
        foreach ($arg in @('build', '--release', '--locked', '--offline', '--bin', 'ficant-server')) {
            [void]$buildPsi.ArgumentList.Add($arg)
        }
        $buildPsi.Environment['CARGO_TARGET_DIR'] = $phase1Target
        $buildPsi.Environment['FICANT_CODE_COMMIT_SHA'] = $CommitSha
        $buildPsi.Environment['FICANT_CODE_TREE_SHA'] = $TreeSha
        foreach ($clearVar in @('FICANT_EMBED_WEB_DIST_DIR', 'FICANT_STATIC_WEB_DIR', 'FICANT_SERVE_WEB_UI', 'FICANT_SERVER_ADDR')) {
            [void]$buildPsi.Environment.Remove($clearVar)
        }
        foreach ($key in @($buildPsi.Environment.Keys)) {
            if ($key.StartsWith('FICANT_BLOTTER_', [System.StringComparison]::OrdinalIgnoreCase) -or
                $key.StartsWith('FICANT_SPI_', [System.StringComparison]::OrdinalIgnoreCase)) {
                [void]$buildPsi.Environment.Remove($key)
            }
        }

        $buildProc = [System.Diagnostics.Process]::Start($buildPsi)
        if ($null -eq $buildProc) {
            throw 'Failed to start Phase 1 cargo release build.'
        }
        $buildProc.WaitForExit()
        $buildExit = $buildProc.ExitCode
        $buildProc.Dispose()
        if ($buildExit -ne 0) {
            throw "Phase 1 isolated 8-path cargo release build failed with exit code $buildExit."
        }

        $binaryName = if ($IsWindows) { 'ficant-server.exe' } else { 'ficant-server' }
        $phase1Bin = Join-Path (Join-Path $phase1Target 'release') $binaryName
        if (-not (Test-Path -LiteralPath $phase1Bin -PathType Leaf)) {
            throw "Phase 1 release binary missing: $phase1Bin"
        }

        $port = Get-FreeLoopbackPort
        $listenAddr = "127.0.0.1:$port"

        $srvPsi = [System.Diagnostics.ProcessStartInfo]::new($phase1Bin)
        $srvPsi.WorkingDirectory = $phase1RunDir
        $srvPsi.UseShellExecute = $false
        $srvPsi.RedirectStandardInput = $true
        $srvPsi.RedirectStandardOutput = $true
        $srvPsi.RedirectStandardError = $true
        $srvPsi.Environment['FICANT_SERVER_ADDR'] = $listenAddr
        $srvPsi.Environment['FICANT_ENABLE_STDIN_SHUTDOWN'] = '1'
        foreach ($clearVar in @('FICANT_EMBED_WEB_DIST_DIR', 'FICANT_STATIC_WEB_DIR', 'FICANT_SERVE_WEB_UI')) {
            [void]$srvPsi.Environment.Remove($clearVar)
        }
        foreach ($key in @($srvPsi.Environment.Keys)) {
            if ($key.StartsWith('FICANT_BLOTTER_', [System.StringComparison]::OrdinalIgnoreCase) -or
                $key.StartsWith('FICANT_SPI_', [System.StringComparison]::OrdinalIgnoreCase)) {
                [void]$srvPsi.Environment.Remove($key)
            }
        }

        $proc = [System.Diagnostics.Process]::Start($srvPsi)
        if ($null -eq $proc) {
            throw 'Failed to start Phase 1 ficant-server binary.'
        }
        $stderrTask = $proc.StandardError.ReadToEndAsync()

        $httpClient = [System.Net.Http.HttpClient]::new()
        $httpClient.Timeout = [TimeSpan]::FromSeconds(2)
        try {
            $deadline = [DateTime]::UtcNow.AddSeconds(15)
            $healthResp = $null
            $healthBody = ''
            while ([DateTime]::UtcNow -lt $deadline) {
                if ($proc.HasExited) {
                    $errOut = $stderrTask.GetAwaiter().GetResult()
                    throw "Phase 1 ficant-server exited prematurely (code=$($proc.ExitCode)): $errOut"
                }
                try {
                    $resp = $httpClient.GetAsync("http://$listenAddr/healthz").GetAwaiter().GetResult()
                    if ([int]$resp.StatusCode -eq 200) {
                        $healthResp = $resp
                        $healthBody = $resp.Content.ReadAsStringAsync().GetAwaiter().GetResult()
                        break
                    }
                    $resp.Dispose()
                }
                catch {
                    Start-Sleep -Milliseconds 100
                }
            }

            if ($null -eq $healthResp) {
                throw "Timed out waiting for Phase 1 /healthz on http://$listenAddr/healthz."
            }

            try {
                foreach ($hdr in @($healthResp.Headers) + @($healthResp.Content.Headers)) {
                    if ($hdr.Key.StartsWith('Access-Control-Allow-', [System.StringComparison]::OrdinalIgnoreCase)) {
                        throw "Phase 1 /healthz returned forbidden CORS header: $($hdr.Key)"
                    }
                }
            }
            finally {
                $healthResp.Dispose()
            }

            $healthJson = $healthBody | ConvertFrom-Json
            if ($healthJson.status -cne 'ok') { throw "Phase 1 /healthz status expected 'ok', got '$($healthJson.status)'" }
            if ($healthJson.service -cne 'ficant-server') { throw "Phase 1 /healthz service expected 'ficant-server', got '$($healthJson.service)'" }
            if ($healthJson.storage_mode -cne 'disabled') { throw "Phase 1 /healthz storage_mode expected 'disabled', got '$($healthJson.storage_mode)'" }
            if ($healthJson.blotter_healthy -ne $true) { throw "Phase 1 /healthz blotter_healthy expected true, got '$($healthJson.blotter_healthy)'" }
            if ($null -ne $healthJson.ledger_epoch) { throw "Phase 1 /healthz ledger_epoch expected null, got '$($healthJson.ledger_epoch)'" }
            if ($null -ne $healthJson.ledger_instance_id) { throw "Phase 1 /healthz ledger_instance_id expected null, got '$($healthJson.ledger_instance_id)'" }
            if ($healthJson.web_ui_mode -cne 'disabled') { throw "Phase 1 /healthz web_ui_mode expected 'disabled', got '$($healthJson.web_ui_mode)'" }
            if ([int]$healthJson.embedded_assets_count -ne 0) { throw "Phase 1 /healthz embedded_assets_count expected 0, got '$($healthJson.embedded_assets_count)'" }

            # Verify --health-check CLI exits 0 against the exact same newly linked binary.
            $hcPsi = [System.Diagnostics.ProcessStartInfo]::new($phase1Bin)
            $hcPsi.WorkingDirectory = $phase1RunDir
            $hcPsi.UseShellExecute = $false
            [void]$hcPsi.ArgumentList.Add('--health-check')
            $hcPsi.Environment['FICANT_SERVER_ADDR'] = $listenAddr
            $hcProc = [System.Diagnostics.Process]::Start($hcPsi)
            if ($null -eq $hcProc) {
                throw 'Failed to start Phase 1 --health-check process.'
            }
            if (-not $hcProc.WaitForExit(10000)) {
                $hcProc.Kill($true)
                $hcProc.Dispose()
                throw 'Phase 1 --health-check timed out after 10s.'
            }
            $hcExit = $hcProc.ExitCode
            $hcProc.Dispose()
            if ($hcExit -ne 0) {
                throw "Phase 1 --health-check failed with exit code $hcExit."
            }

            $shutdownExit = Stop-ServerProcessGracefully -Process $proc -TimeoutMs 10000
            if ($shutdownExit -ne 0) {
                throw "Phase 1 ficant-server exited with non-zero code $shutdownExit on SHUTDOWN."
            }

            Write-Host ("Phase 1 PASS: 8-path Dockerfile release build linked & booted cleanly; /healthz={0}; --health-check exit=0; SHUTDOWN exit=0." -f $healthBody.Trim())
        }
        finally {
            $httpClient.Dispose()
        }
    }
    finally {
        if ($null -ne $proc) {
            if (-not $proc.HasExited) {
                try { $proc.Kill($true) } catch {}
                try { [void]$proc.WaitForExit(3000) } catch {}
            }
            $proc.Dispose()
        }
        Remove-TempSubtreeSafely -TempRoot $TempRoot -TargetDir $phase1Root -ExpectedLeafRegex '^ficant-bp4-phase1-[0-9a-f]{32}$'
    }
}

function Invoke-Phase2EmbeddedSingleBinaryE2E {
    param(
        [Parameter(Mandatory)]
        [string]$TempRoot,

        [switch]$SkipBrowser
    )

    Write-Host ''
    Write-Host '==> Phase 2: Isolated embedded single-binary packaging & zero-source / zero-dist E2E (F-14, AC-001, AC-010)'

    $runId = [Guid]::NewGuid().ToString('N')
    $phase2Root = [System.IO.Path]::GetFullPath((Join-Path $TempRoot "ficant-bp4-phase2-$runId"))
    $phase2PkgOut = Join-Path $phase2Root 'pkg-out'
    $phase2CargoTarget = Join-Path $phase2Root 'cargo-target'
    $phase2RuntimeDir = Join-Path $phase2Root 'clean-runtime'
    $evidencePath = Join-Path $phase2Root 'bp4-e2e-evidence.json'

    [void](New-Item -ItemType Directory -Path $phase2Root -Force)
    [void](New-Item -ItemType Directory -Path $phase2RuntimeDir -Force)

    $envKeys = @(
        'FICANT_BP4_SERVER_BIN',
        'FICANT_BP4_REQUIRE_BROWSER_E2E',
        'FICANT_BP4_EVIDENCE_OUT'
    )
    $savedEnv = @{}
    foreach ($k in $envKeys) {
        $savedEnv[$k] = [System.Environment]::GetEnvironmentVariable($k, 'Process')
    }

    try {
        $pkgArgs = @(
            '-NoProfile',
            '-NonInteractive',
            '-File',
            (Join-Path $PSScriptRoot 'package-intranet.ps1'),
            '-OutputDir',
            $phase2PkgOut,
            '-CargoTargetDir',
            $phase2CargoTarget
        )
        & pwsh @pkgArgs
        if ($LASTEXITCODE -ne 0) {
            throw "Phase 2 package-intranet.ps1 failed with exit code $LASTEXITCODE."
        }

        $binaryName = if ($IsWindows) { 'ficant-server.exe' } else { 'ficant-server' }
        $packagedBin = Join-Path $phase2PkgOut $binaryName
        $packagedSha = "$packagedBin.sha256"
        if (-not (Test-Path -LiteralPath $packagedBin -PathType Leaf)) {
            throw "Packaged binary missing: $packagedBin"
        }
        if (-not (Test-Path -LiteralPath $packagedSha -PathType Leaf)) {
            throw "Packaged SHA-256 manifest missing: $packagedSha"
        }

        $shaLine = (Get-Content -LiteralPath $packagedSha -Raw).Trim()
        $actualSha = (Get-FileHash -LiteralPath $packagedBin -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($shaLine -cne "$actualSha  $binaryName") {
            throw "Packaged .sha256 content mismatch: got '$shaLine', expected '$actualSha  $binaryName'"
        }

        $cleanRuntimeBin = Join-Path $phase2RuntimeDir $binaryName
        [System.IO.File]::Copy($packagedBin, $cleanRuntimeBin, $false)

        # Delete intermediate packaging output & cargo target dirs BEFORE launching E2E.
        Remove-Item -LiteralPath $phase2PkgOut -Recurse -Force
        Remove-Item -LiteralPath $phase2CargoTarget -Recurse -Force

        $runtimeEntries = @(Get-ChildItem -LiteralPath $phase2RuntimeDir -Force)
        if ($runtimeEntries.Count -ne 1 -or $runtimeEntries[0].Name -cne $binaryName) {
            throw "Clean runtime directory must contain ONLY $binaryName."
        }

        [System.Environment]::SetEnvironmentVariable('FICANT_BP4_SERVER_BIN', $cleanRuntimeBin, 'Process')
        [System.Environment]::SetEnvironmentVariable('FICANT_BP4_REQUIRE_BROWSER_E2E', $(if ($SkipBrowser) { '0' } else { '1' }), 'Process')
        [System.Environment]::SetEnvironmentVariable('FICANT_BP4_EVIDENCE_OUT', $evidencePath, 'Process')

        Push-Location -LiteralPath $script:FicantRoot
        try {
            $pytestArgs = @('run', '--offline', '--locked', '--project', 'python', 'python', '-m', 'pytest', '-q', 'python/tests/test_bp4_intranet_e2e.py')
            & uv @pytestArgs
            $pytestExit = $LASTEXITCODE
        }
        finally {
            Pop-Location
        }
        if ($pytestExit -ne 0) {
            throw "Phase 2 E2E pytest failed with exit code $pytestExit."
        }

        if (Test-Path -LiteralPath $evidencePath -PathType Leaf) {
            $evidenceText = (Get-Content -LiteralPath $evidencePath -Raw).Trim()
            Write-Host ("Phase 2 PASS (binary sha256={0}): evidence={1}" -f $actualSha, $evidenceText)
        }
        else {
            Write-Host ("Phase 2 PASS (binary sha256={0})." -f $actualSha)
        }
    }
    finally {
        foreach ($k in $envKeys) {
            [System.Environment]::SetEnvironmentVariable($k, $savedEnv[$k], 'Process')
        }
        Remove-TempSubtreeSafely -TempRoot $TempRoot -TargetDir $phase2Root -ExpectedLeafRegex '^ficant-bp4-phase2-[0-9a-f]{32}$'
    }
}

function Parse-ExternalPackagesFromCargoLock {
    param(
        [Parameter(Mandatory)]
        [string]$Text
    )

    $blocks = $Text -split '(?m)^\[\[package\]\]\s*$'
    $set = [System.Collections.Generic.SortedSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($blk in $blocks) {
        if ([string]::IsNullOrWhiteSpace($blk)) { continue }
        $name = if ($blk -match '(?m)^name\s*=\s*"([^"]+)"') { $Matches[1] } else { '' }
        $ver = if ($blk -match '(?m)^version\s*=\s*"([^"]+)"') { $Matches[1] } else { '' }
        $src = if ($blk -match '(?m)^source\s*=\s*"([^"]+)"') { $Matches[1] } else { '' }
        $chk = if ($blk -match '(?m)^checksum\s*=\s*"([^"]+)"') { $Matches[1] } else { '' }
        if (-not [string]::IsNullOrEmpty($src)) {
            [void]$set.Add("$name|$ver|$src|$chk")
        }
    }
    return ,$set
}

function Invoke-Phase3GovernanceChecks {
    Write-Host ''
    Write-Host '==> Phase 3: Lint, layering & frozen-scope governance (AC-011)'

    Push-Location -LiteralPath $script:FicantRoot
    try {
        & pnpm --dir apps/desk-web run lint
        if ($LASTEXITCODE -ne 0) {
            throw "Phase 3 desk-web ESLint failed with exit code $LASTEXITCODE."
        }

        & pwsh -NoProfile -NonInteractive -File (Join-Path $PSScriptRoot 'check-layering.ps1')
        if ($LASTEXITCODE -ne 0) {
            throw "Phase 3 layering gate failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }

    [void](Invoke-GitRaw -ArgumentList @('-C', $script:FicantRoot, 'rev-parse', '--verify', '--quiet', "$($script:Bp4BaselineCommit)^{commit}"))

    $violations = [System.Collections.Generic.List[string]]::new()
    $diffSpecs = @(
        [pscustomobject]@{ Name = 'diff(BASE..HEAD)'; Args = @('-C', $script:FicantRoot, 'diff', '--no-renames', '--name-status', '-z', $script:Bp4BaselineCommit, 'HEAD', '--') + $script:Bp4FrozenRoots }
        [pscustomobject]@{ Name = 'diff(--cached BASE)'; Args = @('-C', $script:FicantRoot, 'diff', '--cached', '--no-renames', '--name-status', '-z', $script:Bp4BaselineCommit, '--') + $script:Bp4FrozenRoots }
        [pscustomobject]@{ Name = 'diff(BASE..worktree)'; Args = @('-C', $script:FicantRoot, 'diff', '--no-renames', '--name-status', '-z', $script:Bp4BaselineCommit, '--') + $script:Bp4FrozenRoots }
    )
    foreach ($spec in $diffSpecs) {
        $raw = Invoke-GitRaw -ArgumentList $spec.Args
        $tokens = @(Split-NulTokens -Raw $raw -Label $spec.Name)
        if ($tokens.Count -gt 0) {
            $violations.Add("$($spec.Name): frozen paths changed -> $($tokens -join ' ')")
        }
    }

    $statusRaw = Invoke-GitRaw -ArgumentList (@('-C', $script:FicantRoot, '--no-optional-locks', 'status', '--porcelain=v1', '-z', '-uall', '--') + $script:Bp4FrozenRoots)
    $statusTokens = @(Split-NulTokens -Raw $statusRaw -Label 'status --porcelain=v1')
    if ($statusTokens.Count -gt 0) {
        $violations.Add("status: frozen paths have uncommitted or untracked files -> $($statusTokens -join ' ')")
    }

    if ($violations.Count -gt 0) {
        throw "Frozen-scope check failed relative to $($script:Bp4BaselineCommit):`n$($violations -join "`n")"
    }

    # Verify Cargo.lock external crate-set parity relative to baseline.
    $baseCargoLock = Invoke-GitRaw -ArgumentList @('-C', $script:FicantRoot, 'show', "$($script:Bp4BaselineCommit):Cargo.lock")
    $curCargoLock = Get-Content -LiteralPath (Join-Path $script:FicantRoot 'Cargo.lock') -Raw
    $baseSet = Parse-ExternalPackagesFromCargoLock -Text $baseCargoLock
    $curSet = Parse-ExternalPackagesFromCargoLock -Text $curCargoLock
    $added = @($curSet | Where-Object { -not $baseSet.Contains($_) })
    $removed = @($baseSet | Where-Object { -not $curSet.Contains($_) })
    if ($added.Count -gt 0 -or $removed.Count -gt 0) {
        throw "Cargo.lock external crate set drifted vs $($script:Bp4BaselineCommit)! Added=[$($added -join '; ')] Removed=[$($removed -join '; ')]"
    }

    # Verify python/uv.lock byte-for-byte parity.
    $baseUvLock = Invoke-GitRaw -ArgumentList @('-C', $script:FicantRoot, 'show', "$($script:Bp4BaselineCommit):python/uv.lock")
    $curUvLock = [System.IO.File]::ReadAllText((Join-Path $script:FicantRoot 'python\uv.lock'), [System.Text.UTF8Encoding]::new($false))
    if ($curUvLock -cne $baseUvLock) {
        throw "python/uv.lock drifted relative to $($script:Bp4BaselineCommit)."
    }

    Write-Host ("Phase 3 PASS: desk-web lint=0; check-layering=0; {0} frozen roots clean; {1} Cargo.lock external crates identical; python/uv.lock byte-identical vs {2}." -f $script:Bp4FrozenRoots.Count, $curSet.Count, $script:Bp4BaselineCommit)
}

$planSteps = @(
    New-FicantCheckStep -Name 'Phase 1: Isolated 8-path RustService.Dockerfile release build & disabled-UI boot proof' -FilePath 'cargo' -ArgumentList @('build', '--release', '--locked', '--offline', '--bin', 'ficant-server')
    New-FicantCheckStep -Name 'Phase 2a: Package embedded single-binary via package-intranet.ps1' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/package-intranet.ps1')
    New-FicantCheckStep -Name $(if ($SkipBrowserE2E) { 'Phase 2b: Clean-runtime E2E (PARTIAL: -SkipBrowserE2E)' } else { 'Phase 2b: Clean-runtime single-binary + Chromium E2E (AC-001, AC-010)' }) -FilePath 'uv' -ArgumentList @('run', '--offline', '--locked', '--project', 'python', 'python', '-m', 'pytest', '-q', 'python/tests/test_bp4_intranet_e2e.py')
    New-FicantCheckStep -Name 'Phase 3a: Desk Web ESLint (including verify-single-binary-ui.mjs)' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'run', 'lint')
    New-FicantCheckStep -Name 'Phase 3b: Layering gate (AC03=0, AC01=0, allowlist=0)' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/check-layering.ps1')
    New-FicantCheckStep -Name "Phase 3c: Frozen-scope & Cargo.lock/uv.lock parity vs $script:Bp4BaselineCommit" -FilePath 'git' -ArgumentList (@('diff', '--no-renames', '--name-status', $script:Bp4BaselineCommit, '--') + $script:Bp4FrozenRoots)
)

try {
    if ($ListOnly) {
        Show-FicantCheckPlan -Steps $planSteps
        if ($SkipBrowserE2E) {
            Write-Host 'Note: -SkipBrowserE2E produces PARTIAL output and does NOT satisfy AC-010.'
        }
        exit 0
    }

    Assert-FicantRustCapability
    [void](Assert-FicantMinimumVersion -Name 'node' -ArgumentList @('--version') -Minimum '22.13.0')
    [void](Assert-FicantMinimumVersion -Name 'pnpm' -ArgumentList @('--version') -Minimum '11.0.0')
    [void](Assert-FicantMinimumVersion -Name 'uv' -ArgumentList @('--version') -Minimum '0.7.0' -Pinned '0.7.13')
    Assert-FicantCommand 'git'
    Assert-FicantCommand 'pwsh'

    $commitSha = Get-GitSha40 -RevParseArgs @('HEAD') -Label 'commit SHA'
    $treeSha = Get-GitSha40 -RevParseArgs @('HEAD^{tree}') -Label 'tree SHA'
    $tempRoot = (Get-Item -LiteralPath ([System.IO.Path]::GetTempPath()) -ErrorAction Stop).FullName

    Invoke-Phase1DockerfileSubsetProof -TempRoot $tempRoot -CommitSha $commitSha -TreeSha $treeSha
    Invoke-Phase2EmbeddedSingleBinaryE2E -TempRoot $tempRoot -SkipBrowser:$SkipBrowserE2E
    Invoke-Phase3GovernanceChecks

    Write-Host ''
    if ($SkipBrowserE2E) {
        Write-Host 'PARTIAL: -SkipBrowserE2E was specified; browser verification skipped and AC-010 is NOT satisfied.'
    }
    else {
        Write-Host 'FICANT BP-4 intranet full verification passed (Phase 1 + Phase 2 + Phase 3; AC-001, AC-010, AC-011 satisfied).'
    }
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
