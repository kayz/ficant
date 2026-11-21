[CmdletBinding()]
param(
    [switch]$ListOnly,
    [switch]$SkipE2E,
    [switch]$ScopeCheckOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$script:Bp3BaselineCommit = 'b701c5622133358168abe27b080d11f87aeb0a69'
$script:Bp3ExactAllowedPaths = @(
    '.gitignore',
    'scripts/check-desk-web.ps1',
    'docs/development.md'
)

function Invoke-Bp3GitRaw {
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

function Split-Bp3NulTokens {
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
        throw "BP-3 write-scope check ($Label) received non-NUL-terminated git output."
    }
    return @($Raw.Substring(0, $Raw.Length - 1).Split([char]0))
}

function Test-Bp3AllowedPath {
    param(
        [Parameter(Mandatory)]
        [string]$RelativePath
    )

    if ($RelativePath -in $script:Bp3ExactAllowedPaths) {
        return $true
    }
    if ($RelativePath.StartsWith('apps/desk-web/', [System.StringComparison]::Ordinal)) {
        return $true
    }
    return $false
}

function Test-Bp3ForbiddenSecretPath {
    param(
        [Parameter(Mandatory)]
        [string]$RelativePath
    )

    if (-not $RelativePath.StartsWith('apps/desk-web/', [System.StringComparison]::Ordinal)) {
        return $false
    }
    if ($RelativePath -imatch '(^|/)\.env') {
        return $true
    }
    if ($RelativePath -imatch '\.key$') {
        return $true
    }
    return $false
}

function Add-Bp3PathViolationIfInvalid {
    param(
        [Parameter(Mandatory)]
        [AllowEmptyCollection()]
        [System.Collections.Generic.List[string]]$Violations,

        [Parameter(Mandatory)]
        [string]$LayerName,

        [Parameter(Mandatory)]
        [string]$Status,

        [Parameter(Mandatory)]
        [string]$RelativePath
    )

    if (-not (Test-Bp3AllowedPath -RelativePath $RelativePath)) {
        $Violations.Add("${LayerName}: out-of-scope path '$RelativePath' (status '$Status')")
        return
    }
    if (Test-Bp3ForbiddenSecretPath -RelativePath $RelativePath) {
        $Violations.Add("${LayerName}: forbidden secret/env file '$RelativePath' (status '$Status')")
    }
}

function Invoke-Bp3WriteScopeCheck {
    param(
        [Parameter(Mandatory)]
        [string]$RepositoryRoot,

        [Parameter(Mandatory)]
        [string]$BaseCommit
    )

    Assert-FicantCommand 'git'
    $repoRootFull = [System.IO.Path]::GetFullPath($RepositoryRoot)
    [void](Invoke-Bp3GitRaw -ArgumentList @('-C', $repoRootFull, 'rev-parse', '--verify', '--quiet', "$BaseCommit^{commit}"))

    $violations = [System.Collections.Generic.List[string]]::new()

    $diffLayers = @(
        [pscustomobject]@{
            Name = 'diff(BASE..HEAD)'
            Args = @('-C', $repoRootFull, 'diff', '--find-renames', '--name-status', '-z', $BaseCommit, 'HEAD')
        },
        [pscustomobject]@{
            Name = 'diff(--cached BASE)'
            Args = @('-C', $repoRootFull, 'diff', '--cached', '--find-renames', '--name-status', '-z', $BaseCommit)
        },
        [pscustomobject]@{
            Name = 'diff(BASE..worktree)'
            Args = @('-C', $repoRootFull, 'diff', '--find-renames', '--name-status', '-z', $BaseCommit)
        }
    )

    foreach ($layer in $diffLayers) {
        $raw = Invoke-Bp3GitRaw -ArgumentList $layer.Args
        $tokens = @(Split-Bp3NulTokens -Raw $raw -Label $layer.Name)
        $index = 0
        while ($index -lt $tokens.Count) {
            $status = $tokens[$index]
            if ($status.StartsWith('R', [System.StringComparison]::Ordinal) -or
                $status.StartsWith('C', [System.StringComparison]::Ordinal)) {
                if ($index + 2 -ge $tokens.Count) {
                    throw "Malformed rename/copy record in $($layer.Name) at token $index."
                }
                $oldPath = $tokens[$index + 1]
                $newPath = $tokens[$index + 2]
                Add-Bp3PathViolationIfInvalid -Violations $violations -LayerName $layer.Name -Status $status -RelativePath $oldPath
                Add-Bp3PathViolationIfInvalid -Violations $violations -LayerName $layer.Name -Status $status -RelativePath $newPath
                $index += 3
                continue
            }
            if ($index + 1 -ge $tokens.Count) {
                throw "Malformed name-status record in $($layer.Name) at token $index."
            }
            $path = $tokens[$index + 1]
            Add-Bp3PathViolationIfInvalid -Violations $violations -LayerName $layer.Name -Status $status -RelativePath $path
            $index += 2
        }
    }

    $statusRaw = Invoke-Bp3GitRaw -ArgumentList @('-C', $repoRootFull, '--no-optional-locks', 'status', '--porcelain=v1', '-z', '-uall')
    $statusTokens = @(Split-Bp3NulTokens -Raw $statusRaw -Label 'status --porcelain=v1')
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
            if ($index + 1 -ge $statusTokens.Count) {
                throw "Malformed porcelain rename/copy record at token $index."
            }
            $origPath = $statusTokens[$index + 1]
            Add-Bp3PathViolationIfInvalid -Violations $violations -LayerName 'status' -Status $xy -RelativePath $origPath
            Add-Bp3PathViolationIfInvalid -Violations $violations -LayerName 'status' -Status $xy -RelativePath $path
            $index += 2
            continue
        }

        Add-Bp3PathViolationIfInvalid -Violations $violations -LayerName 'status' -Status $xy -RelativePath $path
        $index += 1
    }

    # Disk scan under apps/desk-web (excluding build/cache dirs) for gitignored .env* or *.key files.
    $deskWebRoot = Join-Path $repoRootFull 'apps\desk-web'
    if (Test-Path -LiteralPath $deskWebRoot -PathType Container) {
        $diskFiles = Get-ChildItem -LiteralPath $deskWebRoot -File -Recurse -Force -ErrorAction Stop | Where-Object {
            $rel = [System.IO.Path]::GetRelativePath($repoRootFull, $_.FullName).Replace('\', '/')
            $rel -notmatch '(?i)^apps/desk-web/(node_modules|dist|coverage|\.vite)(/|$)'
        }
        foreach ($file in @($diskFiles)) {
            $rel = [System.IO.Path]::GetRelativePath($repoRootFull, $file.FullName).Replace('\', '/')
            if (Test-Bp3ForbiddenSecretPath -RelativePath $rel) {
                $violations.Add("disk: forbidden secret/env file present on disk '$rel'")
            }
        }

        foreach ($disallowedPlaywrightDir in @('test-results', 'playwright-report')) {
            $candidate = Join-Path $deskWebRoot $disallowedPlaywrightDir
            if (Test-Path -LiteralPath $candidate) {
                $violations.Add("disk: Playwright output leaked outside coverage/playwright-results: 'apps/desk-web/$disallowedPlaywrightDir'")
            }
        }
    }

    if ($violations.Count -gt 0) {
        $details = ($violations | ForEach-Object { "  - $_" }) -join [Environment]::NewLine
        throw "BP-3 repository write-scope check failed with $($violations.Count) violation(s) relative to ${BaseCommit}:`n$details"
    }

    Write-Host "BP-3 repository write-scope check passed (base=$BaseCommit, 0 out-of-scope paths, 0 secret/env files)."
}

$baseSteps = @(
    New-FicantCheckStep -Name 'Desk Web frozen install' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'install', '--frozen-lockfile')
    New-FicantCheckStep -Name 'Desk Web Strategy A codegen check' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'run', 'check:codegen')
    New-FicantCheckStep -Name 'Desk Web TypeScript check' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'run', 'typecheck')
    New-FicantCheckStep -Name 'Desk Web ESLint' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'run', 'lint')
    New-FicantCheckStep -Name 'Desk Web Vitest suite' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'run', 'test')
    New-FicantCheckStep -Name 'Desk Web production build' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'run', 'build')
)

$scopeCheckStep = New-FicantCheckStep -Name 'BP-3 repository write-scope check' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/check-desk-web.ps1', '-ScopeCheckOnly')

$steps = if ($SkipE2E) {
    @($baseSteps + $scopeCheckStep)
}
else {
    @(
        $baseSteps + @(
            New-FicantCheckStep -Name 'Desk Web production bundle mount smoke' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'run', 'smoke:mount', '--', 'desk')
            New-FicantCheckStep -Name 'Rust ficant-server check' -FilePath 'cargo' -ArgumentList @('check', '--locked', '-p', 'ficant-server')
            New-FicantCheckStep -Name 'Rust ficant-server build' -FilePath 'cargo' -ArgumentList @('build', '--locked', '-p', 'ficant-server')
            New-FicantCheckStep -Name 'Desk Web real-server Playwright E2E' -FilePath 'pnpm' -ArgumentList @('--dir', 'apps/desk-web', 'run', 'test:e2e')
            $scopeCheckStep
        )
    )
}

try {
    if ($ListOnly -and $ScopeCheckOnly) {
        throw 'Cannot specify both -ListOnly and -ScopeCheckOnly.'
    }

    if ($ScopeCheckOnly) {
        Invoke-Bp3WriteScopeCheck -RepositoryRoot $script:FicantRoot -BaseCommit $script:Bp3BaselineCommit
        exit 0
    }

    if ($ListOnly) {
        Show-FicantCheckPlan -Steps $steps
        exit 0
    }

    [void](Assert-FicantMinimumVersion -Name 'node' -ArgumentList @('--version') -Minimum '22.13.0')
    [void](Assert-FicantMinimumVersion -Name 'pnpm' -ArgumentList @('--version') -Minimum '11.0.0')
    [void](Assert-FicantMinimumVersion -Name 'buf' -ArgumentList @('--version') -Minimum '1.56.0' -Pinned '1.56.0')
    if (-not $SkipE2E) {
        Assert-FicantRustCapability
    }

    Invoke-FicantCheckPlan -Steps $steps
    Write-Host ''
    if ($SkipE2E) {
        Write-Host 'FICANT desk-web fast checks (-SkipE2E) passed.'
    }
    else {
        Write-Host 'FICANT desk-web full checks (including real-server E2E) passed.'
    }
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
