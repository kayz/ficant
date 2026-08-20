[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$checker = Join-Path $PSScriptRoot 'check-manual.ps1'
$scratchRoot = Join-Path (
    [System.IO.Path]::GetTempPath()
) ("ficant-manual-test-{0}" -f [Guid]::NewGuid().ToString('N'))

$validManual = @'
# Fixture MANUAL

<!-- ficant-manual-literal: dev-up -->
```powershell
.\scripts\dev-up.ps1 -ListOnly
.\scripts\dev-up.ps1
```

<!-- ficant-manual-literal: dev-down -->
```powershell
.\scripts\dev-down.ps1
```

<!-- ficant-manual-literal: check-fast -->
```powershell
.\scripts\check-fast.ps1 -ListOnly
.\scripts\check-fast.ps1
```

<!-- ficant-manual-literal: check-full -->
```powershell
.\scripts\check.ps1
```

<!-- ficant-manual-literal: check-integration -->
```powershell
.\scripts\check.ps1 -IncludeIntegration
```

<!-- ficant-manual-literal: recovery-proof -->
```powershell
.\scripts\check-recovery.ps1 -ListOnly
.\scripts\check-recovery.ps1
```
'@

function Invoke-Native {
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,

        [Parameter(Mandatory)]
        [string[]]$ArgumentList
    )

    & $FilePath @ArgumentList | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath failed with exit code $LASTEXITCODE."
    }
}

function New-AuthorityFixture {
    param(
        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Manual
    )

    $root = Join-Path $scratchRoot $Name
    New-Item -ItemType Directory -Path $root | Out-Null
    [System.IO.File]::WriteAllText(
        (Join-Path $root 'SPEC.md'),
        "fixture spec`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    [System.IO.File]::WriteAllText(
        (Join-Path $root 'ACCEPTANCE.md'),
        "fixture acceptance`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    [System.IO.File]::WriteAllText(
        (Join-Path $root 'MANUAL.md'),
        $Manual.TrimStart() + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    $documents = foreach ($path in @('SPEC.md', 'ACCEPTANCE.md', 'MANUAL.md')) {
        [ordered]@{
            path = $path
            sha256 = (Get-FileHash -LiteralPath (Join-Path $root $path) -Algorithm SHA256).Hash
        }
    }
    $manifest = [ordered]@{
        schema = 'ficant.authority.snapshot.v1'
        captured_at = '2026-08-19T00:00:00Z'
        capture_status = 'active_authority'
        public_repository = 'https://example.invalid/ficant'
        public_code_commit = '1111111111111111111111111111111111111111'
        documents = @($documents)
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $root 'authority-manifest.json'),
        ($manifest | ConvertTo-Json -Depth 6) + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    Invoke-Native -FilePath 'git' -ArgumentList @('-C', $root, 'init', '-b', 'main')
    Invoke-Native -FilePath 'git' -ArgumentList @('-C', $root, 'add', '--all')
    Invoke-Native -FilePath 'git' -ArgumentList @(
        '-C', $root,
        '-c', 'user.name=FICANT Manual Fixture',
        '-c', 'user.email=manual-fixture@invalid.example',
        'commit', '-m', 'fixture'
    )
    $commit = (& git -C $root rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $commit -notmatch '^[0-9a-f]{40}$') {
        throw "Unable to resolve fixture commit '$Name'."
    }
    return [pscustomobject]@{
        Root = $root
        Commit = $commit
    }
}

function Invoke-Checker {
    param(
        [Parameter(Mandatory)]
        [object]$Fixture,

        [string]$ExpectedCommit = ''
    )

    if ([string]::IsNullOrWhiteSpace($ExpectedCommit)) {
        $ExpectedCommit = $Fixture.Commit
    }
    $output = @(& pwsh -NoProfile -NonInteractive -File $checker `
        -AuthorityRoot $Fixture.Root `
        -ExpectedAuthorityCommit $ExpectedCommit `
        -ListOnly 2>&1)
    return [pscustomobject]@{
        ExitCode = $LASTEXITCODE
        Output = $output -join "`n"
    }
}

function Assert-Pass {
    param(
        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [object]$Fixture
    )

    $result = Invoke-Checker -Fixture $Fixture
    if ($result.ExitCode -ne 0) {
        throw "Expected '$Name' to pass, exit=$($result.ExitCode): $($result.Output)"
    }
}

function Assert-Fail {
    param(
        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [object]$Fixture,

        [string]$ExpectedCommit = ''
    )

    $result = Invoke-Checker -Fixture $Fixture -ExpectedCommit $ExpectedCommit
    if ($result.ExitCode -eq 0) {
        throw "Expected '$Name' to fail."
    }
}

try {
    New-Item -ItemType Directory -Path $scratchRoot | Out-Null

    $good = New-AuthorityFixture -Name 'good' -Manual $validManual
    Assert-Pass -Name 'exact marker set' -Fixture $good

    $unmarked = New-AuthorityFixture -Name 'unmarked' -Manual (
        $validManual -replace (
            '(?m)^<!-- ficant-manual-literal: dev-up -->\r?\n',
            ''
        )
    )
    Assert-Fail -Name 'unmarked PowerShell block' -Fixture $unmarked

    $duplicate = New-AuthorityFixture -Name 'duplicate' -Manual (
        $validManual.Replace(
            '<!-- ficant-manual-literal: check-full -->',
            '<!-- ficant-manual-literal: check-fast -->'
        )
    )
    Assert-Fail -Name 'duplicate block id' -Fixture $duplicate

    $placeholder = New-AuthorityFixture -Name 'placeholder' -Manual (
        $validManual.Replace('.\scripts\check.ps1', '.\scripts\check.ps1 -Input <replace-me>')
    )
    Assert-Fail -Name 'placeholder' -Fixture $placeholder

    $forbidden = New-AuthorityFixture -Name 'forbidden' -Manual (
        $validManual.Replace('.\scripts\check.ps1', "git push origin main`n.\scripts\check.ps1")
    )
    Assert-Fail -Name 'remote mutation' -Fixture $forbidden

    Assert-Fail -Name 'authority commit drift' -Fixture $good `
        -ExpectedCommit '0000000000000000000000000000000000000000'

    $dirty = New-AuthorityFixture -Name 'dirty' -Manual $validManual
    Add-Content -LiteralPath (Join-Path $dirty.Root 'SPEC.md') -Value 'drift'
    Assert-Fail -Name 'dirty authority checkout' -Fixture $dirty

    Write-Output 'MANUAL checker fixture tests passed: 1 positive, 6 negative.'
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
finally {
    if (Test-Path -LiteralPath $scratchRoot) {
        $resolvedScratch = [System.IO.Path]::GetFullPath($scratchRoot)
        $tempPrefix = [System.IO.Path]::GetFullPath(
            [System.IO.Path]::GetTempPath()
        ).TrimEnd([System.IO.Path]::DirectorySeparatorChar) +
            [System.IO.Path]::DirectorySeparatorChar
        if (-not $resolvedScratch.StartsWith(
            $tempPrefix,
            [System.StringComparison]::OrdinalIgnoreCase
        ) -or [System.IO.Path]::GetFileName($resolvedScratch) -notlike 'ficant-manual-test-*') {
            throw "Refusing to remove unexpected fixture root '$resolvedScratch'."
        }
        Remove-Item -LiteralPath $resolvedScratch -Recurse -Force
    }
}
