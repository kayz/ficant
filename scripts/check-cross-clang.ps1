[CmdletBinding()]
param(
    [switch]$CompareOnly,
    [string]$WindowsOutput,
    [string]$LinuxOutput
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

$windowsCompiler = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\clang++.exe'
$linuxDistribution = 'ficant-ubuntu-24.04'
$linuxCompiler = '/usr/bin/clang++-18'

function Read-ValidatedManifest {
    param([Parameter(Mandatory)][string]$Path)

    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
    $lines = @(Get-Content -LiteralPath $resolved)
    if ($lines.Count -eq 0) {
        throw "Raw numeric manifest is empty: $resolved"
    }

    $keys = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($line in $lines) {
        $parts = $line -split "`t", 2
        if ($parts.Count -ne 2) {
            throw "Raw numeric manifest row is not tab-separated: $line"
        }
        $key = $parts[0]
        $value = $parts[1]
        if ($key -notmatch '^[fi]\.[A-Za-z0-9_.\[\]-]+$') {
            throw "Raw numeric manifest key is invalid: $key"
        }
        if (-not $keys.Add($key)) {
            throw "Raw numeric manifest key is duplicated: $key"
        }
        if ($key.StartsWith('f.', [System.StringComparison]::Ordinal)) {
            if ($value -notmatch '^[0-9a-f]{16}$') {
                throw "Floating-point output is not one lowercase IEEE-754 bit pattern: $line"
            }
            $bits = [UInt64]::Parse(
                $value,
                [Globalization.NumberStyles]::HexNumber,
                [Globalization.CultureInfo]::InvariantCulture
            )
            if (($bits -band [UInt64]0x7ff0000000000000) -eq [UInt64]0x7ff0000000000000) {
                throw "Floating-point output is non-finite: $line"
            }
        }
        elseif ($value -notmatch '^(0|-?[1-9][0-9]*)$') {
            throw "Integer output is not canonical decimal: $line"
        }
    }
    return $lines
}

function Compare-RawNumericManifests {
    param(
        [Parameter(Mandatory)][string]$WindowsPath,
        [Parameter(Mandatory)][string]$LinuxPath
    )

    $windowsLines = @(Read-ValidatedManifest -Path $WindowsPath)
    $linuxLines = @(Read-ValidatedManifest -Path $LinuxPath)
    if ($windowsLines.Count -ne $linuxLines.Count) {
        throw "Cross-Clang output count differs: Windows=$($windowsLines.Count), Linux=$($linuxLines.Count)."
    }
    for ($index = 0; $index -lt $windowsLines.Count; $index++) {
        if ($windowsLines[$index] -cne $linuxLines[$index]) {
            throw "Cross-Clang raw output differs at row $($index + 1). Windows='$($windowsLines[$index])'; Linux='$($linuxLines[$index])'."
        }
    }
    Write-Host "Cross-Clang raw numeric manifests are identical ($($windowsLines.Count) rows)."
}

function Invoke-CheckedExternal {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]]$ArgumentList,
        [switch]$Capture
    )

    $output = @(& $FilePath @ArgumentList 2>&1)
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "Command failed (${exitCode}): $FilePath $($ArgumentList -join ' ')`n$($output -join [Environment]::NewLine)"
    }
    if ($Capture) {
        return @($output | ForEach-Object { $_.ToString() })
    }
    if ($output.Count -gt 0) {
        $output | ForEach-Object { Write-Host $_ }
    }
}

try {
    if ($CompareOnly) {
        if ([string]::IsNullOrWhiteSpace($WindowsOutput) -or [string]::IsNullOrWhiteSpace($LinuxOutput)) {
            throw 'CompareOnly requires both -WindowsOutput and -LinuxOutput.'
        }
        Compare-RawNumericManifests -WindowsPath $WindowsOutput -LinuxPath $LinuxOutput
        exit 0
    }
    if (-not [string]::IsNullOrWhiteSpace($WindowsOutput) -or -not [string]::IsNullOrWhiteSpace($LinuxOutput)) {
        throw '-WindowsOutput and -LinuxOutput are only valid with -CompareOnly.'
    }

    Assert-FicantCommand $windowsCompiler
    Assert-FicantCommand 'wsl.exe'
    $windowsVersion = @(Invoke-CheckedExternal -FilePath $windowsCompiler -ArgumentList @('--version') -Capture)
    if ($windowsVersion[0].Trim() -ne 'clang version 19.1.5') {
        throw "Required Windows compiler is clang version 19.1.5, got '$($windowsVersion[0].Trim())'."
    }
    $linuxVersion = @(Invoke-CheckedExternal -FilePath 'wsl.exe' -ArgumentList @(
            '-d', $linuxDistribution, '--', $linuxCompiler, '--version'
        ) -Capture)
    $expectedLinuxVersion = 'Ubuntu clang version 18.1.8 (++20240731025043+3b5b5c1ec4a3-1~exp1~20240731145144.92)'
    if ($linuxVersion[0].Trim() -ne $expectedLinuxVersion) {
        throw "Required Linux compiler is '$expectedLinuxVersion', got '$($linuxVersion[0].Trim())'."
    }

    $tempBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    $runId = [Guid]::NewGuid().ToString('N')
    $tempRoot = [System.IO.Path]::GetFullPath((Join-Path $tempBase "ficant-r7a-cross-clang-$runId"))
    if (-not $tempRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to create output outside the temporary directory: $tempRoot"
    }
    New-Item -ItemType Directory -Path $tempRoot | Out-Null
    $linuxTempRoot = "/tmp/ficant-r7a-cross-clang-$runId"
    if ($linuxTempRoot -notmatch '^/tmp/ficant-r7a-cross-clang-[0-9a-f]{32}$') {
        throw "Refusing to use unexpected Linux temporary directory: $linuxTempRoot"
    }
    Invoke-CheckedExternal -FilePath 'wsl.exe' -ArgumentList @(
        '-d', $linuxDistribution, '--', '/usr/bin/mkdir', '-p', $linuxTempRoot
    )

    $runnerPath = Join-Path $script:FicantRoot 'cpp\fixed-income-kernel\tests\r7a_raw_numeric.cpp'
    $includePath = Join-Path $script:FicantRoot 'cpp\fixed-income-kernel\include'
    $sources = @(Get-ChildItem -LiteralPath (Join-Path $script:FicantRoot 'cpp\fixed-income-kernel\src') -Filter '*.cpp' -File | Sort-Object Name)
    if (-not (Test-Path -LiteralPath $runnerPath -PathType Leaf) -or $sources.Count -eq 0) {
        throw 'R7A numeric runner or production C++ sources are missing.'
    }

    $windowsExecutable = Join-Path $tempRoot 'r7a-raw-numeric.exe'
    $commonFlags = @('-std=c++20', '-O2', '-DNDEBUG', '-DFICANT_KERNEL_BUILD', '-Wall', '-Wextra', '-Wpedantic', '-Werror')
    $windowsArguments = @($commonFlags) + @('-I', $includePath, $runnerPath) + @($sources.FullName) + @('-o', $windowsExecutable)
    Invoke-CheckedExternal -FilePath $windowsCompiler -ArgumentList $windowsArguments

    $windowsRootForWsl = $script:FicantRoot.Replace('\', '/')
    $wslRootOutput = @(Invoke-CheckedExternal -FilePath 'wsl.exe' -ArgumentList @(
            '-d', $linuxDistribution, '--', '/usr/bin/wslpath', '-a', $windowsRootForWsl
        ) -Capture)
    $wslRoot = $wslRootOutput[0].TrimEnd('/')
    if (-not $wslRoot.StartsWith('/', [System.StringComparison]::Ordinal)) {
        throw "Could not resolve the repository in WSL: $wslRoot"
    }
    $wslRunner = "$wslRoot/cpp/fixed-income-kernel/tests/r7a_raw_numeric.cpp"
    $wslInclude = "$wslRoot/cpp/fixed-income-kernel/include"
    $wslSources = @($sources | ForEach-Object {
            $relative = [System.IO.Path]::GetRelativePath($script:FicantRoot, $_.FullName).Replace('\', '/')
            "$wslRoot/$relative"
        })
    $linuxExecutable = "$linuxTempRoot/r7a-raw-numeric"
    $linuxArguments = @('-d', $linuxDistribution, '--', $linuxCompiler) + @($commonFlags) +
        @('-I', $wslInclude, $wslRunner) + $wslSources + @('-o', $linuxExecutable)
    Invoke-CheckedExternal -FilePath 'wsl.exe' -ArgumentList $linuxArguments

    $windowsLines = @(Invoke-CheckedExternal -FilePath $windowsExecutable -ArgumentList @() -Capture)
    $linuxLines = @(Invoke-CheckedExternal -FilePath 'wsl.exe' -ArgumentList @(
            '-d', $linuxDistribution, '--', $linuxExecutable
        ) -Capture)
    $utf8 = [System.Text.UTF8Encoding]::new($false)
    $windowsManifest = Join-Path $tempRoot 'windows-clang-19.tsv'
    $linuxManifest = Join-Path $tempRoot 'ubuntu-clang-18.tsv'
    [System.IO.File]::WriteAllLines($windowsManifest, [string[]]$windowsLines, $utf8)
    [System.IO.File]::WriteAllLines($linuxManifest, [string[]]$linuxLines, $utf8)
    Compare-RawNumericManifests -WindowsPath $windowsManifest -LinuxPath $linuxManifest
    $digest = (Get-FileHash -LiteralPath $windowsManifest -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "Cross-Clang raw numeric manifest SHA-256: $digest"
    exit 0
}
catch {
    Write-Error $_
    exit 1
}
finally {
    if (-not $CompareOnly) {
        if ($null -ne (Get-Variable -Name linuxTempRoot -ErrorAction SilentlyContinue) -and
            $linuxTempRoot -match '^/tmp/ficant-r7a-cross-clang-[0-9a-f]{32}$') {
            & wsl.exe -d $linuxDistribution -- /usr/bin/rm -rf -- $linuxTempRoot 2>$null
            if ($LASTEXITCODE -ne 0) {
                Write-Warning "Could not remove Linux temporary directory: $linuxTempRoot"
            }
        }
        if ($null -ne (Get-Variable -Name tempRoot -ErrorAction SilentlyContinue) -and
            (Test-Path -LiteralPath $tempRoot -PathType Container)) {
            $resolvedRoot = (Resolve-Path -LiteralPath $tempRoot).Path
            if (-not $resolvedRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase) -or
                (Split-Path -Leaf $resolvedRoot) -notmatch '^ficant-r7a-cross-clang-[0-9a-f]{32}$') {
                throw "Refusing to remove unexpected Windows temporary directory: $resolvedRoot"
            }
            Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
        }
    }
}
