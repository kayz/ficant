[CmdletBinding()]
param(
    [switch]$BuildAndValidate,
    [string]$RunnerPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

if ($BuildAndValidate -and -not [string]::IsNullOrWhiteSpace($RunnerPath)) {
    throw '-BuildAndValidate and -RunnerPath cannot be specified together.'
}

$gatePath = Join-Path $PSScriptRoot 'check-cross-clang.ps1'
$tempBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$tempRoot = [System.IO.Path]::GetFullPath((Join-Path $tempBase ('ficant-cross-clang-fixtures-' + [Guid]::NewGuid().ToString('N'))))
if (-not $tempRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to create fixtures outside the temporary directory: $tempRoot"
}
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)

function Invoke-Comparator {
    param(
        [Parameter(Mandatory)][string]$WindowsOutput,
        [Parameter(Mandatory)][string]$LinuxOutput,
        [Parameter(Mandatory)][int]$ExpectedExitCode,
        [Parameter(Mandatory)][string]$Scenario,
        [string]$ExpectedErrorPattern
    )

    $output = @(& pwsh -NoProfile -NonInteractive -File $gatePath -CompareOnly `
            -WindowsOutput $WindowsOutput -LinuxOutput $LinuxOutput 2>&1)
    $actualExitCode = $LASTEXITCODE
    $joinedOutput = ($output | ForEach-Object { $_.ToString() }) -join [Environment]::NewLine
    if ($actualExitCode -ne $ExpectedExitCode) {
        throw "Scenario '$Scenario' expected exit code $ExpectedExitCode but got $actualExitCode.`n$joinedOutput"
    }
    if (-not [string]::IsNullOrWhiteSpace($ExpectedErrorPattern) -and
        $joinedOutput -notmatch $ExpectedErrorPattern) {
        throw "Scenario '$Scenario' exited $actualExitCode as expected, but output did not match '$ExpectedErrorPattern'.`n$joinedOutput"
    }
}

function Invoke-CheckedStep {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$FilePath,
        [Parameter(Mandatory)][AllowEmptyCollection()][string[]]$ArgumentList
    )

    Write-Host "==> $Name"
    & $FilePath @ArgumentList
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "Step '$Name' failed with exit code ${exitCode}: $FilePath $($ArgumentList -join ' ')"
    }
}

function Invoke-AndValidateRunner {
    param(
        [Parameter(Mandatory)][string]$BinaryPath,
        [Parameter(Mandatory)][string]$TempDirectory
    )

    if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
        throw "Raw numeric runner binary was not found: $BinaryPath"
    }
    $resolvedRunner = (Resolve-Path -LiteralPath $BinaryPath -ErrorAction Stop).Path
    $stderrFile = Join-Path $TempDirectory 'runner-stderr.log'
    $lines = @(& $resolvedRunner 2> $stderrFile)
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        $stderrText = Get-Content -LiteralPath $stderrFile -Raw -ErrorAction SilentlyContinue
        throw "Raw numeric runner failed with exit code ${exitCode}: $resolvedRunner`n$stderrText"
    }
    if ($lines.Count -eq 0) {
        throw "Raw numeric runner produced empty output: $resolvedRunner"
    }

    $keys = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($line in $lines) {
        $parts = $line.ToString() -split "`t", 2
        if ($parts.Count -eq 2) {
            [void]$keys.Add($parts[0])
        }
    }
    foreach ($requiredNonHedgeKey in @(
            'i.kernel.abi_version',
            'i.bond.return_status',
            'i.curve.return_status',
            'i.carry_roll.return_status',
            'i.delivery.return_status'
        )) {
        if (-not $keys.Contains($requiredNonHedgeKey)) {
            throw "Raw numeric runner output is missing non-hedge key '$requiredNonHedgeKey': $resolvedRunner"
        }
    }

    $manifestPath = Join-Path $TempDirectory 'runner-stdout.tsv'
    [System.IO.File]::WriteAllLines($manifestPath, [string[]]($lines | ForEach-Object { $_.ToString() }), $utf8NoBom)
    Invoke-Comparator -WindowsOutput $manifestPath -LinuxOutput $manifestPath `
        -ExpectedExitCode 0 -Scenario "real runner manifest validation ($resolvedRunner)"
    Write-Host "Validated real runner output ($($lines.Count) rows): $resolvedRunner"
}

try {
    New-Item -ItemType Directory -Path $tempRoot | Out-Null

    $nonHedgeRows = @(
        "i.kernel.abi_version`t1",
        "i.bond.return_status`t0",
        "i.bond.cashflow_count`t2",
        "f.bond.clean_price`t4059000000000000",
        "i.delivery.return_status`t0",
        "f.delivery.conversion_factor`t3fee666666666666"
    )
    $hedge1mRows = @(
        "i.hedge.return_status`t0",
        "i.hedge.struct_size`t56",
        "i.hedge.abi_version`t1",
        "i.hedge.status_code`t0",
        "i.hedge.recommended_contracts`t-1",
        "f.hedge.futures_contract_dv01`t407f400000000000",
        "f.hedge.raw_contracts`tbff0000000000000",
        "f.hedge.residual_dv01`t0000000000000000",
        "f.hedge.hedge_effectiveness`t3ff0000000000000"
    )
    $hedge2mRows = @(
        "i.hedge_2m.return_status`t0",
        "i.hedge_2m.struct_size`t56",
        "i.hedge_2m.abi_version`t1",
        "i.hedge_2m.status_code`t0",
        "i.hedge_2m.recommended_contracts`t-4",
        "f.hedge_2m.futures_contract_dv01`t4072260a38800000",
        "f.hedge_2m.raw_contracts`tc011016582a00000",
        "f.hedge_2m.residual_dv01`t40523f6090000000",
        "f.hedge_2m.hedge_effectiveness`t3fee186a50000000"
    )

    $completeBaseline = @($nonHedgeRows + $hedge1mRows + $hedge2mRows)
    $windowsComplete = Join-Path $tempRoot 'windows-complete.tsv'
    $linuxComplete = Join-Path $tempRoot 'linux-complete.tsv'
    [System.IO.File]::WriteAllLines($windowsComplete, $completeBaseline, $utf8NoBom)
    [System.IO.File]::WriteAllLines($linuxComplete, $completeBaseline, $utf8NoBom)

    # 1. Positive case: identical complete manifests (non-hedge + hedge + hedge_2m) pass.
    Invoke-Comparator -WindowsOutput $windowsComplete -LinuxOutput $linuxComplete `
        -ExpectedExitCode 0 -Scenario 'identical complete manifests pass'

    # 2. Negative case: both sides omit the entire hedge_2m group.
    $missingGroupRows = @($nonHedgeRows + $hedge1mRows)
    $windowsMissingGroup = Join-Path $tempRoot 'windows-missing-2m-group.tsv'
    $linuxMissingGroup = Join-Path $tempRoot 'linux-missing-2m-group.tsv'
    [System.IO.File]::WriteAllLines($windowsMissingGroup, $missingGroupRows, $utf8NoBom)
    [System.IO.File]::WriteAllLines($linuxMissingGroup, $missingGroupRows, $utf8NoBom)
    Invoke-Comparator -WindowsOutput $windowsMissingGroup -LinuxOutput $linuxMissingGroup `
        -ExpectedExitCode 1 -Scenario 'both sides missing hedge_2m group fails' `
        -ExpectedErrorPattern "missing required hedge group 'hedge_2m'"

    # 3. Negative case: both sides omit one required float key in hedge_2m.
    $missingFloatKeyRows = @($completeBaseline | Where-Object { $_ -notlike "f.hedge_2m.futures_contract_dv01`t*" })
    $windowsMissingFloat = Join-Path $tempRoot 'windows-missing-float-key.tsv'
    $linuxMissingFloat = Join-Path $tempRoot 'linux-missing-float-key.tsv'
    [System.IO.File]::WriteAllLines($windowsMissingFloat, $missingFloatKeyRows, $utf8NoBom)
    [System.IO.File]::WriteAllLines($linuxMissingFloat, $missingFloatKeyRows, $utf8NoBom)
    Invoke-Comparator -WindowsOutput $windowsMissingFloat -LinuxOutput $linuxMissingFloat `
        -ExpectedExitCode 1 -Scenario 'both sides missing required hedge_2m float key fails' `
        -ExpectedErrorPattern 'missing required key\(s\) for hedge group ''hedge_2m''.*f\.hedge_2m\.futures_contract_dv01'

    # 4. Negative case: both sides omit one required integer key in hedge.
    $missingIntKeyRows = @($completeBaseline | Where-Object { $_ -notlike "i.hedge.recommended_contracts`t*" })
    $windowsMissingInt = Join-Path $tempRoot 'windows-missing-int-key.tsv'
    $linuxMissingInt = Join-Path $tempRoot 'linux-missing-int-key.tsv'
    [System.IO.File]::WriteAllLines($windowsMissingInt, $missingIntKeyRows, $utf8NoBom)
    [System.IO.File]::WriteAllLines($linuxMissingInt, $missingIntKeyRows, $utf8NoBom)
    Invoke-Comparator -WindowsOutput $windowsMissingInt -LinuxOutput $linuxMissingInt `
        -ExpectedExitCode 1 -Scenario 'both sides missing required hedge integer key fails' `
        -ExpectedErrorPattern 'missing required key\(s\) for hedge group ''hedge''.*i\.hedge\.recommended_contracts'

    # 5. Negative case: single-bit IEEE-754 drift on hedge_2m row on Linux side.
    $driftedHedgeRows = @($completeBaseline | ForEach-Object {
            if ($_ -eq "f.hedge_2m.futures_contract_dv01`t4072260a38800000") {
                "f.hedge_2m.futures_contract_dv01`t4072260a38800001"
            }
            else {
                $_
            }
        })
    $linuxDriftedHedge = Join-Path $tempRoot 'linux-drifted-hedge-2m.tsv'
    [System.IO.File]::WriteAllLines($linuxDriftedHedge, $driftedHedgeRows, $utf8NoBom)
    Invoke-Comparator -WindowsOutput $windowsComplete -LinuxOutput $linuxDriftedHedge `
        -ExpectedExitCode 1 -Scenario 'one IEEE-754 bit drift on hedge_2m fails' `
        -ExpectedErrorPattern 'Cross-Clang raw output differs at row'

    # 6. Negative case: single-bit IEEE-754 drift on non-hedge row on Linux side.
    $driftedBondRows = @($completeBaseline | ForEach-Object {
            if ($_ -eq "f.bond.clean_price`t4059000000000000") {
                "f.bond.clean_price`t4059000000000001"
            }
            else {
                $_
            }
        })
    $linuxDriftedBond = Join-Path $tempRoot 'linux-drifted-bond.tsv'
    [System.IO.File]::WriteAllLines($linuxDriftedBond, $driftedBondRows, $utf8NoBom)
    Invoke-Comparator -WindowsOutput $windowsComplete -LinuxOutput $linuxDriftedBond `
        -ExpectedExitCode 1 -Scenario 'one IEEE-754 bit drift on non-hedge row fails' `
        -ExpectedErrorPattern 'Cross-Clang raw output differs at row'

    # 7. Negative case: unexpected extra key inside hedge_2m group.
    $extraKeyRows = @($completeBaseline + @("f.hedge_2m.unexpected_extra`t0000000000000000"))
    $windowsExtra = Join-Path $tempRoot 'windows-extra-key.tsv'
    $linuxExtra = Join-Path $tempRoot 'linux-extra-key.tsv'
    [System.IO.File]::WriteAllLines($windowsExtra, $extraKeyRows, $utf8NoBom)
    [System.IO.File]::WriteAllLines($linuxExtra, $extraKeyRows, $utf8NoBom)
    Invoke-Comparator -WindowsOutput $windowsExtra -LinuxOutput $linuxExtra `
        -ExpectedExitCode 1 -Scenario 'unexpected key in hedge_2m group fails' `
        -ExpectedErrorPattern "unexpected key for hedge group 'hedge_2m': f\.hedge_2m\.unexpected_extra"

    # 8. Negative case: duplicate key rejected.
    $duplicateKeyRows = @($completeBaseline + @("i.hedge.return_status`t0"))
    $windowsDup = Join-Path $tempRoot 'windows-dup-key.tsv'
    $linuxDup = Join-Path $tempRoot 'linux-dup-key.tsv'
    [System.IO.File]::WriteAllLines($windowsDup, $duplicateKeyRows, $utf8NoBom)
    [System.IO.File]::WriteAllLines($linuxDup, $duplicateKeyRows, $utf8NoBom)
    Invoke-Comparator -WindowsOutput $windowsDup -LinuxOutput $linuxDup `
        -ExpectedExitCode 1 -Scenario 'duplicated key fails' `
        -ExpectedErrorPattern 'duplicated: i\.hedge\.return_status'

    # 9. Negative case: non-finite IEEE-754 bit pattern rejected.
    $nonFiniteRows = @($completeBaseline | ForEach-Object {
            if ($_ -eq "f.hedge.residual_dv01`t0000000000000000") {
                "f.hedge.residual_dv01`t7ff0000000000000"
            }
            else {
                $_
            }
        })
    $windowsNonFinite = Join-Path $tempRoot 'windows-nonfinite.tsv'
    $linuxNonFinite = Join-Path $tempRoot 'linux-nonfinite.tsv'
    [System.IO.File]::WriteAllLines($windowsNonFinite, $nonFiniteRows, $utf8NoBom)
    [System.IO.File]::WriteAllLines($linuxNonFinite, $nonFiniteRows, $utf8NoBom)
    Invoke-Comparator -WindowsOutput $windowsNonFinite -LinuxOutput $linuxNonFinite `
        -ExpectedExitCode 1 -Scenario 'non-finite float bit pattern fails' `
        -ExpectedErrorPattern 'non-finite'

    Write-Host 'Cross-Clang comparator fixture tests passed (9 assertions).'

    if (-not [string]::IsNullOrWhiteSpace($RunnerPath)) {
        Invoke-AndValidateRunner -BinaryPath $RunnerPath -TempDirectory $tempRoot
    }
    elseif ($BuildAndValidate) {
        foreach ($command in @('cmake', 'ctest', 'ninja')) {
            Assert-FicantCommand $command
        }

        $visualStudioClang = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\clang++.exe'
        $cppCompiler = if (-not [string]::IsNullOrWhiteSpace($env:FICANT_CXX)) {
            $env:FICANT_CXX
        }
        elseif (Test-Path -LiteralPath $visualStudioClang -PathType Leaf) {
            $visualStudioClang
        }
        else {
            $pathClang = Get-Command 'clang++' -ErrorAction SilentlyContinue
            if ($null -eq $pathClang -or [string]::IsNullOrWhiteSpace($pathClang.Source)) {
                throw "Required command 'clang++' was not found. No dependency will be installed automatically."
            }
            $pathClang.Source
        }

        [void](Assert-FicantMinimumVersion -Name 'clang++' -FilePath $cppCompiler -ArgumentList @('--version') -Minimum ([version]'18.0.0') -Pinned '19.1.5')

        $cppSourceDirectory = Join-Path $script:FicantRoot 'cpp\fixed-income-kernel'
        $cppBuildDirectory = Join-Path $script:FicantRoot 'build\local-cpp-vs-llvm-19'
        $cmakeConfigureArgs = @(
            '-S', $cppSourceDirectory,
            '-B', $cppBuildDirectory,
            '-G', 'Ninja',
            "-DCMAKE_CXX_COMPILER=$cppCompiler",
            '-DCMAKE_BUILD_TYPE=Release'
        )

        $compilerDirectory = $null
        if ([System.IO.Path]::IsPathRooted($cppCompiler)) {
            $compilerDirectory = Split-Path -Parent $cppCompiler
        }
        else {
            $resolvedCompiler = Get-Command $cppCompiler -ErrorAction SilentlyContinue
            if ($null -ne $resolvedCompiler -and -not [string]::IsNullOrWhiteSpace($resolvedCompiler.Source)) {
                $compilerDirectory = Split-Path -Parent $resolvedCompiler.Source
            }
        }
        if ($IsWindows -and -not [string]::IsNullOrWhiteSpace($compilerDirectory)) {
            $llvmRc = Join-Path $compilerDirectory 'llvm-rc.exe'
            if (Test-Path -LiteralPath $llvmRc -PathType Leaf) {
                $cmakeConfigureArgs += "-DCMAKE_RC_COMPILER=$($llvmRc.Replace('\', '/'))"
            }
        }

        Invoke-CheckedStep -Name 'C++ configure (Ninja Release)' -FilePath 'cmake' -ArgumentList $cmakeConfigureArgs
        Invoke-CheckedStep -Name 'C++ build (--parallel)' -FilePath 'cmake' -ArgumentList @('--build', $cppBuildDirectory, '--parallel')
        Invoke-CheckedStep -Name 'C++ tests (ctest)' -FilePath 'ctest' -ArgumentList @('--test-dir', $cppBuildDirectory, '--output-on-failure')

        $runnerName = if ($IsWindows) { 'r7a_raw_numeric.exe' } else { 'r7a_raw_numeric' }
        $builtRunner = Join-Path $cppBuildDirectory $runnerName
        Invoke-AndValidateRunner -BinaryPath $builtRunner -TempDirectory $tempRoot
    }

    exit 0
}
catch {
    Write-Error $_
    exit 1
}
finally {
    if (Test-Path -LiteralPath $tempRoot -PathType Container) {
        $resolvedRoot = (Resolve-Path -LiteralPath $tempRoot).Path
        if (-not $resolvedRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase) -or
            (Split-Path -Leaf $resolvedRoot) -notmatch '^ficant-cross-clang-fixtures-[0-9a-f]{32}$') {
            throw "Refusing to remove unexpected fixture root: $resolvedRoot"
        }
        Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
    }
}
