[CmdletBinding()]
param(
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'check-common.ps1')

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
    New-FicantCheckStep -Name 'Cross-Clang raw numeric identity' -FilePath 'pwsh' -ArgumentList @('-NoProfile', '-NonInteractive', '-File', 'scripts/check-cross-clang.ps1')
    New-FicantCheckStep -Name 'Phase 2B independent Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_phase2b_manual_oracle.py', '-q')
    New-FicantCheckStep -Name 'Phase 2C independent Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_phase2c_manual_oracle.py', '-q')
    New-FicantCheckStep -Name 'Phase 2D independent Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_phase2d_manual_oracle.py', '-q')
    New-FicantCheckStep -Name 'Portfolio KRD independent Decimal Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_r5d_portfolio_krd_decimal_oracle.py', '-q')
    New-FicantCheckStep -Name 'Tax-adjusted independent Decimal Oracle' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', 'tests/oracle/china-rates/test_r5e_tax_adjusted_decimal_oracle.py', '-q')
    New-FicantCheckStep -Name 'Python contracts and live SDK' -FilePath 'uv' -ArgumentList @('run', '--locked', '--project', 'python', 'python', '-m', 'pytest', '-q', 'python/tests')
)

try {
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
