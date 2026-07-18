Set-StrictMode -Version Latest

function Get-Sha256Text([string]$Text) {
    $bytes = [Text.Encoding]::UTF8.GetBytes($Text)
    'sha256:' + [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
}
function Assert-WindowsAbsolutePath([string]$Path, [string]$Name) {
    if ($Path -notmatch '^[A-Za-z]:\\') { throw "$Name must be a Windows absolute path" }
}
function Test-Contained([string]$Child, [string]$Parent) {
    $childFull = [IO.Path]::GetFullPath($Child).TrimEnd('\')
    $parentFull = [IO.Path]::GetFullPath($Parent).TrimEnd('\')
    $childFull.Equals($parentFull, [StringComparison]::OrdinalIgnoreCase) -or $childFull.StartsWith($parentFull + '\', [StringComparison]::OrdinalIgnoreCase)
}
function Get-CanonicalPath([string]$Path) { [IO.Path]::GetFullPath($Path).TrimEnd('\') }
function Get-ReparsePathComponent([string]$Path, [string]$TrustedRoot) {
    $pathFull=Get-CanonicalPath $Path; $rootFull=Get-CanonicalPath $TrustedRoot
    if(-not(Test-Contained $pathFull $rootFull)){throw 'destination is outside its trusted root'}
    $current=$rootFull
    if(Test-Path -LiteralPath $current){if((Get-Item -LiteralPath $current -Force).Attributes -band [IO.FileAttributes]::ReparsePoint){return $current}}
    $relative=$pathFull.Substring($rootFull.Length).TrimStart('\')
    foreach($component in @($relative -split '\\'|Where-Object{$_})){
        $current=Join-Path $current $component
        if(Test-Path -LiteralPath $current){if((Get-Item -LiteralPath $current -Force).Attributes -band [IO.FileAttributes]::ReparsePoint){return $current}}
    }
    $null
}
function Assert-NoReparsePathComponents([string]$Path, [string]$TrustedRoot) {
    $pathFull=Get-CanonicalPath $Path; $rootFull=Get-CanonicalPath $TrustedRoot
    if(-not(Test-Contained $pathFull $rootFull)){throw 'destination is outside its trusted root'}
    $volumeRoot=[IO.Path]::GetPathRoot($pathFull)
    if(-not $volumeRoot){throw 'path has no Windows volume root'}
    $current=$volumeRoot
    if(Test-Path -LiteralPath $volumeRoot){if((Get-Item -LiteralPath $volumeRoot -Force).Attributes -band [IO.FileAttributes]::ReparsePoint){throw "reparse point is forbidden in destination path: $current"}}
    $relative=$pathFull.Substring($volumeRoot.Length)
    foreach($component in @($relative -split '\\' | Where-Object { $_ })){
        $current=Join-Path $current $component
        if(Test-Path -LiteralPath $current){if((Get-Item -LiteralPath $current -Force).Attributes -band [IO.FileAttributes]::ReparsePoint){throw "reparse point is forbidden in destination path: $current"}}
    }
}
function Assert-CandidatePathsPhysical([string]$Worktree,$Changes) {
    foreach($path in @($Changes.All)){
        $full=Get-CanonicalPath (Join-Path $Worktree $path)
        $reparse=Get-ReparsePathComponent $full $Worktree
        if($reparse){throw "changed candidate path contains a reparse point: $path ($reparse)"}
    }
}
function Remove-SafeWorktreeNode([string]$Path,[string]$Worktree) {
    $full=Get-CanonicalPath $Path
    if(-not(Test-Contained $full $Worktree) -or $full.Equals((Get-CanonicalPath $Worktree),[StringComparison]::OrdinalIgnoreCase)){throw 'refusing to clean a path outside or equal to worktree'}
    $reparse=Get-ReparsePathComponent $full $Worktree
    if($reparse){
        if($reparse.Equals((Get-CanonicalPath $Worktree),[StringComparison]::OrdinalIgnoreCase)){throw 'refusing to remove reparse worktree root'}
        Remove-Item -LiteralPath $reparse -Force -ErrorAction Stop
        if(Get-Item -LiteralPath $reparse -Force -ErrorAction SilentlyContinue){throw "reparse cleanup verification failed: $reparse"}
        return
    }
    $item=Get-Item -LiteralPath $full -Force -ErrorAction SilentlyContinue
    if($null -eq $item){return}
    if($item -is [IO.DirectoryInfo]){
        foreach($child in @(Get-ChildItem -LiteralPath $full -Force -ErrorAction Stop)){Remove-SafeWorktreeNode $child.FullName $Worktree}
    }
    Remove-Item -LiteralPath $full -Force -ErrorAction Stop
}
function Get-EvidenceRoot {
    $configured=[Environment]::GetEnvironmentVariable('FICANT_EVIDENCE_ROOT')
    if(-not $configured){$configured=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'ficant\runner-evidence'}
    Assert-WindowsAbsolutePath $configured 'FICANT_EVIDENCE_ROOT'
    Get-CanonicalPath $configured
}
function Split-Nul([string]$Text) { @($Text.Split([char]0,[StringSplitOptions]::RemoveEmptyEntries)) }
function Read-JsonObject([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "JSON file not found: $Path" }
    Get-Content -Raw -LiteralPath $Path | ConvertFrom-Json
}
function Get-ClaudeModelIdentity($ParsedOutput) {
    $usage = $ParsedOutput.modelUsage
    if ($null -eq $usage) { throw 'Claude exact provider model identity missing: modelUsage property absent' }
    if (@($usage.PSObject.Properties).Count -eq 0) { throw 'Claude exact provider model identity missing: modelUsage has no entries' }
    $names = @($usage.PSObject.Properties.Name)
    [Array]::Sort($names, [StringComparer]::Ordinal)
    $identities = $names
    if ($identities.Count -eq 0) { throw 'Claude exact provider model identity missing: no identities resolved' }
    $identities -join ','
}
function Get-OrdinalUnique([string[]]$Values) {
    $set = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($value in $Values) { if ($null -ne $value) { [void]$set.Add($value) } }
    $result = @($set)
    [Array]::Sort($result, [StringComparer]::Ordinal)
    $result
}
function Get-CandidateManifest([string]$Worktree) {
    $diff = Get-Git $Worktree @('diff','--binary','--no-ext-diff','HEAD','--')
    $trackedHash = (Get-Sha256Text $diff).Substring(7)
    $ordinary = @(Split-Nul (Get-Git $Worktree @('ls-files','--others','--exclude-standard','-z')))
    $ignored = @(Split-Nul (Get-Git $Worktree @('ls-files','--others','--ignored','--exclude-standard','-z')))
    $untracked = Get-OrdinalUnique @($ordinary + $ignored)
    $entries = @()
    foreach ($path in $untracked) {
        $full = Join-Path $Worktree $path
        $item = Get-Item -LiteralPath $full -Force -ErrorAction Stop
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or -not ($item -is [IO.FileInfo])) {
            throw "unsupported untracked candidate entry: $path"
        }
        $entries += [ordered]@{ path_utf8_base64=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($path)); type='file'; sha256=(Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash.ToLowerInvariant() }
    }
    $manifest = [ordered]@{version=1;tracked_binary_diff_sha256=$trackedHash;untracked=$entries}
    $json = $manifest | ConvertTo-Json -Depth 10 -Compress
    [pscustomobject]@{Manifest=$manifest;Json=$json;Sha256=(Get-Sha256Text $json).Substring(7)}
}
function Get-CapabilityIdentity($Evidence) {
    $identity=[ordered]@{ environment=[string]$Evidence.environment; requested=@($Evidence.requested_capabilities); tools=$Evidence.tools; profile=[string]$Evidence.profile; permission=[string]$Evidence.permission; runner_config_hashes=$Evidence.runner_config_hashes }
    Get-Sha256Text ($identity | ConvertTo-Json -Depth 10 -Compress)
}
function Assert-CapabilityEvidence($Contract,[string]$Root) {
    $path=Get-CanonicalPath ([string]$Contract.capability_evidence_path)
    $evidenceRoot=Get-EvidenceRoot
    $capabilityRoot=Get-CanonicalPath (Join-Path $evidenceRoot 'capabilities')
    if(-not((Split-Path $path -Parent).Equals($capabilityRoot,[StringComparison]::OrdinalIgnoreCase))){throw 'capability evidence file must be a direct child of the runner-owned capabilities directory'}
    Assert-NoReparsePathComponents $path $evidenceRoot
    $evidence=Read-JsonObject $path
    if((Get-CapabilityIdentity $evidence) -ne [string]$evidence.capability_evidence_id -or [string]$evidence.capability_evidence_id -ne [string]$Contract.capability_evidence_id){throw 'capability evidence identity mismatch'}
    if($evidence.status -ne 'ready' -or @($evidence.blockers).Count){throw 'capability evidence is not ready'}
    if($evidence.environment -ne 'windows' -or $evidence.profile -ne $Contract.profile -or $evidence.permission -ne $Contract.permission_profile){throw 'capability evidence context mismatch'}
    $current=(Get-CapabilityEvidence (@($evidence.requested_capabilities) -join ',') $Root $Contract.profile $Contract.permission_profile | ConvertFrom-Json)
    if((Get-CapabilityIdentity $current) -ne [string]$evidence.capability_evidence_id){throw 'capability evidence is stale'}
    [pscustomobject]@{Path=$path;Evidence=$evidence}
}
function Invoke-ArgvProcess([string]$FileName, [string[]]$ArgumentList, [string]$StandardInput = '', [int]$TimeoutSeconds = 120, [string]$WorkingDirectory = '') {
    $info = [System.Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $FileName; $info.UseShellExecute = $false
    $info.RedirectStandardInput = $true; $info.RedirectStandardOutput = $true; $info.RedirectStandardError = $true
    if ($WorkingDirectory) { $info.WorkingDirectory = $WorkingDirectory }
    foreach ($argument in $ArgumentList) { [void]$info.ArgumentList.Add($argument) }
    $process = [System.Diagnostics.Process]::new(); $process.StartInfo = $info
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    if (-not $process.Start()) { throw "Could not start $FileName" }
    $process.StandardInput.Write($StandardInput); $process.StandardInput.Close()
    $stdout = $process.StandardOutput.ReadToEndAsync(); $stderr = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) { $process.Kill($true); $process.WaitForExit(); throw "$FileName timed out" }
    $watch.Stop()
    [pscustomobject]@{ ExitCode=$process.ExitCode; Output=$stdout.Result; Error=$stderr.Result; DurationMs=[int]$watch.ElapsedMilliseconds }
}
function Invoke-Validator([string]$Root, [string[]]$Arguments) {
    $python = (Get-Command python -ErrorAction Stop).Source
    $validator = Join-Path $Root 'execution-validator.py'
    $outcome = Invoke-ArgvProcess $python (@($validator) + $Arguments) '' 60
    if ($outcome.ExitCode -ne 0) { throw "validation failed: $($outcome.Error.Trim())" }
}
function Test-ExecutionConfig([string]$Root) {
    Invoke-Validator $Root @('config','--root',$Root)
    $profiles = Get-Content -Raw -LiteralPath (Join-Path $Root 'profiles.toml')
    if ($profiles -notmatch 'environment = "windows"' -or $profiles -match 'environment = "wsl"') { throw 'ordinary worker environment must be Windows' }
    'WINDOWS_EXECUTION_CONFIG_OK'
}
function Get-Git([string]$Worktree, [string[]]$Arguments) {
    $outcome = Invoke-ArgvProcess 'git' (@('-C',$Worktree) + $Arguments) '' 60
    if ($outcome.ExitCode -ne 0) { throw "git failed: $($outcome.Error.Trim())" }
    $outcome.Output.TrimEnd("`r","`n")
}
function Assert-LinkedWorktree([string]$Worktree) {
    $canonical=Get-CanonicalPath (Resolve-Path -LiteralPath $Worktree).Path
    Assert-NoReparsePathComponents $canonical $canonical
    $gitDir=Get-CanonicalPath (Get-Git $Worktree @('rev-parse','--path-format=absolute','--git-dir'))
    $commonDir=Get-CanonicalPath (Get-Git $Worktree @('rev-parse','--path-format=absolute','--git-common-dir'))
    if($gitDir.Equals($commonDir,[StringComparison]::OrdinalIgnoreCase)){throw 'primary checkout or standalone clone is not an allowed linked worktree'}
    $listed=Get-Git $Worktree @('worktree','list','--porcelain','-z')
    $matches=$false
    foreach($record in (Split-Nul $listed)){if($record.StartsWith('worktree ')){if((Get-CanonicalPath $record.Substring(9)).Equals($canonical,[StringComparison]::OrdinalIgnoreCase)){$matches=$true}}}
    if(-not $matches){throw 'worktree is not registered in git worktree list'}
}
function Assert-EvidenceDestination($Contract) {
    $root=Get-EvidenceRoot; $worktree=Get-CanonicalPath $Contract.worktree
    $evidence=Get-CanonicalPath $Contract.evidence_path; $result=Get-CanonicalPath $Contract.result_path
    if(-not((Split-Path $evidence -Parent).Equals($root,[StringComparison]::OrdinalIgnoreCase))){throw 'evidence_path must be a direct child of FICANT_EVIDENCE_ROOT'}
    if(-not (Split-Path $evidence -Leaf).Equals([string]$Contract.task_id,[StringComparison]::OrdinalIgnoreCase)){throw 'evidence_path child must equal task_id'}
    if(-not(Test-Contained $result $evidence) -or -not((Split-Path $result -Parent).Equals($evidence,[StringComparison]::OrdinalIgnoreCase))){throw 'result_path must be a file directly inside the task evidence directory'}
    if(Test-Contained $root $worktree -or Test-Contained $worktree $root){throw 'evidence root must be outside the worktree'}
    Assert-NoReparsePathComponents $evidence $root
    Assert-NoReparsePathComponents $result $root
    foreach($line in (Split-Nul (Get-Git $Contract.worktree @('worktree','list','--porcelain','-z')))){if($line.StartsWith('worktree ')){ $registered=$line.Substring(9);if((Test-Contained $root $registered) -or (Test-Contained $registered $root)){throw 'evidence root must be outside every registered worktree'}}}
    if(Test-Path -LiteralPath $result){throw 'result_path already exists'}
    if(Test-Path -LiteralPath $evidence){
        if(-not(Test-Path -LiteralPath $evidence -PathType Container)){throw 'evidence_path is not a directory'}
        if((Get-Item -LiteralPath $evidence -Force).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'evidence_path must not be a reparse point'}
        $marker=Join-Path $evidence '.ficant-runner-owned'
        if(-not(Test-Path -LiteralPath $marker -PathType Leaf) -or ([IO.File]::ReadAllText($marker) -ne [string]$Contract.task_id)){throw 'evidence_path is not an existing runner-owned task directory'}
    }
}
function Assert-StructuredCommand($Command, [string]$Worktree) {
    if ($Command.argv -is [string]) { throw 'commands must use structured argv arrays' }
    Assert-WindowsAbsolutePath $Command.cwd 'command cwd'
    if (-not (Test-Contained $Command.cwd $Worktree)) { throw 'command cwd is outside worktree' }
    Assert-NoReparsePathComponents $Command.cwd $Worktree
    $exe = [string]$Command.argv[0]
    if ($exe -match '^(?i)(wsl|wsl\.exe|bash|bash\.exe)$') { throw 'generic Bash/WSL execution is forbidden' }
    if ($exe -match '^(?i)git-bash$') { throw 'git-bash token must be resolved by the runner before execution' }
}
function Get-ReadOnlyCatalogEntry($Command) {
    $catalog=[ordered]@{
        'git-status-porcelain'=@('git','status','--porcelain')
        'git-diff-check'=@('git','diff','--check')
        'git-rev-parse-head'=@('git','rev-parse','HEAD')
    }
    $actual=@($Command.argv | ForEach-Object {[string]$_})
    foreach($identity in $catalog.Keys){
        $expected=$catalog[$identity]
        if($actual.Count -eq $expected.Count){
            $match=$true
            for($i=0;$i -lt $expected.Count;$i++){if(-not $actual[$i].Equals($expected[$i],[StringComparison]::Ordinal)){$match=$false;break}}
            if($match){return [pscustomobject]@{Identity=$identity;Argv=$expected}}
        }
    }
    throw "command is not in the runner-managed read-only catalog: $($actual -join ' ')"
}
function ConvertTo-RepositoryPath([string]$Value,[bool]$AllowSubtreeRule=$false) {
    if([string]::IsNullOrWhiteSpace($Value)){throw 'repository path rule must not be empty'}
    $normalized=$Value.Replace('\','/')
    $subtree=$AllowSubtreeRule -and $normalized.EndsWith('/**',[StringComparison]::Ordinal)
    if($subtree){$normalized=$normalized.Substring(0,$normalized.Length-3)}
    if($normalized.StartsWith('/') -or $normalized -match '^[A-Za-z]:' -or $normalized.EndsWith('/') -or $normalized.Contains('//')){throw "invalid repository path rule: $Value"}
    $segments=@($normalized.Split('/'))
    if($segments.Count -eq 0 -or @($segments|Where-Object{$_ -in @('','.','..') -or $_.Contains('*')}).Count){throw "invalid repository path rule: $Value"}
    [pscustomobject]@{Path=($segments -join '/');Subtree=$subtree}
}
function Test-RepositoryRuleMatch([string]$Path,[string]$Rule) {
    $pathValue=(ConvertTo-RepositoryPath $Path).Path
    $parsed=ConvertTo-RepositoryPath $Rule $true
    $pathValue.Equals($parsed.Path,[StringComparison]::OrdinalIgnoreCase) -or ($parsed.Subtree -and $pathValue.StartsWith($parsed.Path+'/',[StringComparison]::OrdinalIgnoreCase))
}
function Assert-PathRules($Contract) {
    foreach($allowed in @($Contract.allowed_paths)){[void](ConvertTo-RepositoryPath ([string]$allowed) $true)}
    foreach($forbidden in @($Contract.forbidden_paths)){if([string]$forbidden -notmatch '^[A-Za-z]:\\'){[void](ConvertTo-RepositoryPath ([string]$forbidden) $true)}}
}
function Test-Contract([string]$Path, [string]$Root) {
    Invoke-Validator $Root @('instance','--schema',(Join-Path $Root 'schemas\contract.schema.json'),'--instance',$Path)
    $contract = Read-JsonObject $Path
    Assert-WindowsAbsolutePath $contract.worktree 'worktree'; Assert-WindowsAbsolutePath $contract.result_path 'result_path'; Assert-WindowsAbsolutePath $contract.evidence_path 'evidence_path'; Assert-WindowsAbsolutePath $contract.capability_evidence_path 'capability_evidence_path'
    if (-not (Test-Path -LiteralPath $contract.worktree -PathType Container)) { throw 'worktree does not exist' }
    Assert-LinkedWorktree $contract.worktree
    Assert-EvidenceDestination $contract
    $models = @{ strong='gpt-5.6-sol'; fast='gpt-5.3-codex-spark'; medium='sonnet' }
    if ($contract.model.requested -ne $models[[string]$contract.profile]) { throw 'model/profile mismatch' }
    $sandboxes = @{ 'test-executor'='read-only'; quality='read-only'; audit='read-only'; 'test-author'='workspace-write'; development='workspace-write' }
    if (-not $sandboxes.ContainsKey([string]$contract.permission_profile)) { throw 'permission mismatch' }
    $verificationOwners = @{ 'test-executor'='Quality'; 'test-author'='Quality'; development='Orchestrator'; quality='Quality'; audit='Audit' }
    if ($contract.mentor -ne $verificationOwners[[string]$contract.permission_profile]) { throw 'permission/verification owner mismatch' }
    $head = Get-Git $contract.worktree @('rev-parse','HEAD')
    if ($head -ne $contract.base_sha) { throw 'worktree HEAD does not equal exact base SHA' }
    Assert-PathRules $contract
    foreach ($group in @('red','green','regression')) { foreach ($command in $contract.commands.$group) { Assert-StructuredCommand $command $contract.worktree; if($contract.permission_profile -in @('test-executor','quality','audit')){Get-ReadOnlyCatalogEntry $command|Out-Null} } }
    'WINDOWS_EXECUTION_CONTRACT_OK'
}
function Test-Result([string]$Path, [string]$Root) {
    Invoke-Validator $Root @('instance','--schema',(Join-Path $Root 'schemas\result.schema.json'),'--instance',$Path)
    $result = Read-JsonObject $Path
    if ($result.model_identity_source -eq 'unverified') { throw 'actual model identity is unverified' }
    if ($result.requested_sandbox -ne $result.effective_sandbox) { throw 'requested/effective sandbox drift' }
    $profiles=@{strong=@('codex','gpt-5.6-sol','explicit-cli-selection');fast=@('codex','gpt-5.3-codex-spark','explicit-cli-selection');medium=@('claude','sonnet','provider-reported-actual')}
    $expected=$profiles[[string]$result.profile]
    if($result.executor -ne $expected[0] -or $result.requested_model -ne $expected[1] -or $result.model_identity_source -ne $expected[2]){throw 'profile execution identity mismatch'}
    $sandboxes=@{'test-executor'='read-only';quality='read-only';audit='read-only';'test-author'='workspace-write';development='workspace-write'}
    if($result.requested_sandbox -ne $sandboxes[[string]$result.permission_profile]){throw 'permission sandbox mismatch'}
    if($result.requested_sandbox -eq 'read-only' -and $result.command_executor -ne 'runner-managed'){throw 'read-only results require runner-managed commands'}
    if($result.requested_sandbox -eq 'workspace-write' -and $result.command_executor -ne 'worker-direct'){throw 'workspace-write results require worker-direct commands'}
    if($result.environment -ne 'windows'){throw 'result environment must be Windows'}
    if (($result.tests.passed + $result.tests.failed + $result.tests.skipped) -ne $result.tests.total) { throw 'inconsistent test totals' }
    if ($result.candidate_sha -ne $result.base_sha) { throw 'worker commit/base drift is forbidden' }
    foreach ($item in $result.evidence) { if (-not (Test-Contained $item.path $result.evidence_path) -and -not ([string]$item.path).Equals([string]$result.capability_evidence_path,[StringComparison]::OrdinalIgnoreCase)) { throw 'evidence path is outside evidence scope' } }
    'WINDOWS_EXECUTION_RESULT_OK'
}
function Resolve-Tool([string]$Name) {
    if ($Name -eq 'git-bash') {
        $candidates = @((Join-Path $env:ProgramFiles 'Git\bin\bash.exe'), (Join-Path $env:ProgramFiles 'Git\usr\bin\bash.exe'))
        return ($candidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1)
    }
    $command = Get-Command $Name -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -ne $command) { return $command.Source }

    $machineInstallPaths = @{
        cmake = 'C:\Program Files\CMake\bin\cmake.exe'
        ctest = 'C:\Program Files\CMake\bin\ctest.exe'
        clang = 'C:\Program Files\LLVM\bin\clang.exe'
        'clang++' = 'C:\Program Files\LLVM\bin\clang++.exe'
        'clang-cl' = 'C:\Program Files\LLVM\bin\clang-cl.exe'
        'llvm-rc' = 'C:\Program Files\LLVM\bin\llvm-rc.exe'
        'vs-clang' = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\clang.exe'
        'vs-clang++' = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\clang++.exe'
        'vs-clang-cl' = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\clang-cl.exe'
        'vs-llvm-rc' = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\llvm-rc.exe'
    }
    if ($machineInstallPaths.ContainsKey($Name)) {
        $candidate = $machineInstallPaths[$Name]
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
    }
    $null
}
function Get-CapabilityEvidence([string]$RequestedCapabilities, [string]$Root, [string]$Profile, [string]$Permission, [string]$ResultPath = '') {
    $requested = @(Get-OrdinalUnique @($RequestedCapabilities -split ',' | ForEach-Object { $_.Trim().ToLowerInvariant() } | Where-Object { $_ }))
    if ($requested.Count -eq 0) { throw 'at least one requested capability is required' }
    $versionArgs = @{ git=@('--version'); pwsh=@('-NoProfile','-Command','$PSVersionTable.PSVersion.ToString()'); python=@('--version'); codex=@('--version'); claude=@('--version'); rustc=@('--version'); cargo=@('--version'); cmake=@('--version'); ctest=@('--version'); clang=@('--version'); 'clang++'=@('--version'); 'clang-cl'=@('--version'); 'llvm-rc'=@('/?'); 'vs-clang'=@('--version'); 'vs-clang++'=@('--version'); 'vs-clang-cl'=@('--version'); 'vs-llvm-rc'=@('/?'); ninja=@('--version'); 'git-bash'=@('--version') }
    $tools=[ordered]@{}; $blockers=@()
    foreach ($name in $requested) {
        if (-not $versionArgs.ContainsKey($name)) { $blockers += "unsupported requested capability: $name"; continue }
        $path = Resolve-Tool $name
        if (-not $path) { $blockers += "missing requested capability: $name"; continue }
        $observed = Invoke-ArgvProcess $path $versionArgs[$name] '' 30
        if ($observed.ExitCode -ne 0) { $blockers += "version observation failed: $name"; continue }
        $version=(($observed.Output + $observed.Error).Trim())
        if($name -eq 'vs-llvm-rc'){
            $suiteCompiler=Resolve-Tool 'vs-clang'
            $suiteIdentity=Invoke-ArgvProcess $suiteCompiler @('--version') '' 30
            if($suiteIdentity.ExitCode -ne 0){$blockers += 'VS LLVM suite identity observation failed: vs-llvm-rc';continue}
            $suiteFirstLine=(($suiteIdentity.Output+$suiteIdentity.Error) -split '\r?\n'|Select-Object -First 1)
            $version="VS LLVM resource converter companion; $suiteFirstLine"
        }
        $tools[$name]=[ordered]@{ path=$path; version=$version }
    }
    $hashes=[ordered]@{ profile_policy=(Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $Root 'profiles.toml')).Hash.ToLowerInvariant(); capability_policy=(Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $Root 'environment-capabilities.toml')).Hash.ToLowerInvariant(); runner=(Get-FileHash -Algorithm SHA256 -LiteralPath $PSCommandPath).Hash.ToLowerInvariant() }
    $identity=[ordered]@{ environment='windows'; requested=$requested; tools=$tools; profile=$Profile; permission=$Permission; runner_config_hashes=$hashes }
    $result=[ordered]@{ status=$(if($blockers.Count){'blocked'}else{'ready'}); blockers=$blockers; environment='windows'; powershell_version=$PSVersionTable.PSVersion.ToString(); profile=$Profile; permission=$Permission; requested_capabilities=$requested; tools=$tools; runner_config_hashes=$hashes; captured_at=[DateTimeOffset]::UtcNow.ToString('o'); capability_evidence_id=Get-Sha256Text ($identity|ConvertTo-Json -Depth 10 -Compress) }
    $json=$result | ConvertTo-Json -Depth 10 -Compress
    if($ResultPath){
        Assert-WindowsAbsolutePath $ResultPath 'capability ResultPath'
        $capabilityRoot=Get-CanonicalPath (Join-Path (Get-EvidenceRoot) 'capabilities')
        $destination=Get-CanonicalPath $ResultPath
        if(-not((Split-Path $destination -Parent).Equals($capabilityRoot,[StringComparison]::OrdinalIgnoreCase))){throw 'capability ResultPath must be a direct child of runner-owned capabilities directory'}
        Assert-NoReparsePathComponents $destination (Get-EvidenceRoot)
        if(Test-Path -LiteralPath $destination){throw 'capability ResultPath already exists'}
        [IO.Directory]::CreateDirectory((Split-Path $destination -Parent))|Out-Null
        [IO.File]::WriteAllText($destination,$json,[Text.UTF8Encoding]::new($false))
    }
    $json
    if ($blockers.Count) { exit 1 }
}
function Invoke-Preflight([string]$Profile,[string]$Permission,[string]$TestDoublePath) {
    $sandbox = if ($Permission -in @('test-executor','quality','audit')) {'read-only'} else {'workspace-write'}
    $model = @{ strong='gpt-5.6-sol'; fast='gpt-5.3-codex-spark'; medium='sonnet' }[$Profile]
    if ($TestDoublePath -and (Test-Path -LiteralPath $TestDoublePath -PathType Container)) {
        $canary=Join-Path $TestDoublePath 'canary.tmp'
        try { if ($sandbox -eq 'workspace-write') { [IO.File]::WriteAllText($canary,'ficant-canary'); if ([IO.File]::ReadAllText($canary) -ne 'ficant-canary') { throw 'canary mismatch' } } elseif (Test-Path $canary) { throw 'read-only canary existed' } }
        finally { if(Test-Path -LiteralPath $canary){Remove-Item -LiteralPath $canary -Force -ErrorAction Stop};if(Test-Path -LiteralPath $canary){throw 'preflight canary cleanup verification failed'} }
        $actual = if ($Profile -eq 'medium') {'claude-sonnet-test-double'} else {$model}
        return [ordered]@{profile=$Profile;requested_sandbox=$sandbox;effective_sandbox=$sandbox;actual_model_identity=$actual;model_identity_source=$(if($Profile -eq 'medium'){'provider-reported-actual'}else{'explicit-cli-selection'});exit_code=0;cleanup=$true}|ConvertTo-Json -Compress
    }
    $canaryRoot=Join-Path $env:TEMP ("ficant-preflight-"+[guid]::NewGuid().ToString('n')); New-Item -ItemType Directory -Path $canaryRoot|Out-Null
    try {
        $prompt = if($sandbox -eq 'workspace-write'){"Use only your file-edit operation to write exact text ficant-canary to canary.tmp. Do not use shell commands, run tests, retry, or take unrelated actions. Then return selected exact model ID as JSON."}else{"Do not write files. Return selected exact model ID as JSON."}
        if($Profile -eq 'medium') { $args=@('-p','--model',$model,'--permission-mode',$(if($sandbox -eq 'read-only'){'plan'}else{'acceptEdits'}),'--output-format','json','--no-session-persistence'); $out=Invoke-ArgvProcess 'claude' $args $prompt 120 $canaryRoot; $parsed=$out.Output|ConvertFrom-Json; $actual=Get-ClaudeModelIdentity $parsed; if($actual -eq $model){throw 'Claude exact provider model identity missing: identity matches requested alias'}; $source='provider-reported-actual' }
        else { $args=@('exec','--ephemeral','--ignore-user-config','--skip-git-repo-check','-c','windows.sandbox="elevated"','--model',$model,'--sandbox',$sandbox,'--json','-C',$canaryRoot,'-'); $out=Invoke-ArgvProcess 'codex' $args $prompt 120; $actual=$model; $source='explicit-cli-selection' }
        if($out.ExitCode -ne 0){throw "model preflight failed (exit code $($out.ExitCode)): $($out.Error.Trim())"}; $canary=Join-Path $canaryRoot 'canary.tmp'; if($sandbox -eq 'workspace-write' -and (Get-Content -Raw $canary).TrimEnd("`r","`n") -ne 'ficant-canary'){throw 'workspace-write canary failed'}; if($sandbox -eq 'read-only' -and (Test-Path $canary)){throw 'read-only canary failed'}
        [ordered]@{profile=$Profile;requested_sandbox=$sandbox;effective_sandbox=$sandbox;actual_model_identity=$actual;model_identity_source=$source;exit_code=$out.ExitCode;cleanup=$true}|ConvertTo-Json -Compress
    } finally { if(Test-Path -LiteralPath $canaryRoot){Remove-Item -LiteralPath $canaryRoot -Recurse -Force -ErrorAction Stop};if(Test-Path -LiteralPath $canaryRoot){throw 'preflight directory cleanup verification failed'} }
}
function Test-AllowedPath([string]$Path,[object[]]$Allowed,[object[]]$Forbidden=@()) {
    foreach($rule in $Forbidden){if([string]$rule -notmatch '^[A-Za-z]:\\' -and (Test-RepositoryRuleMatch $Path ([string]$rule))){return $false}}
    foreach($rule in $Allowed){if(Test-RepositoryRuleMatch $Path ([string]$rule)){return $true}}
    $false
}
function Invoke-ContractCommand($Command,[string]$Kind,[bool]$ReadOnly) {
    $argv=@($Command.argv); $exe=$argv[0]; if($exe -eq 'git-bash'){$exe=Resolve-Tool 'git-bash';if(-not $exe){throw 'approved git-bash unavailable'}}
    $catalogIdentity=$null;if($ReadOnly){$catalogIdentity=(Get-ReadOnlyCatalogEntry $Command).Identity}
    $out=Invoke-ArgvProcess $exe @($argv|Select-Object -Skip 1) '' $Command.timeout_seconds $Command.cwd
    $observed=0; if($out.Output -match '(?m)tests=(\d+)'){ $observed=[int]$Matches[1] }
    [pscustomobject]@{ Evidence=[ordered]@{kind=$Kind;catalog_identity=$catalogIdentity;argv=$argv;cwd=$Command.cwd;exit_code=$out.ExitCode;duration_ms=$out.DurationMs;expected_tests=[int]$Command.expected_tests;observed_tests=$observed}; Outcome=$out }
}
function Get-WorktreeChanges([string]$Worktree) {
    $tracked=@(Split-Nul (Get-Git $Worktree @('diff','--name-only','--no-renames','-z','HEAD','--')))
    $ordinary=@(Split-Nul (Get-Git $Worktree @('ls-files','--others','--exclude-standard','-z')))
    $ignored=@(Split-Nul (Get-Git $Worktree @('ls-files','--others','--ignored','--exclude-standard','-z')))
    $untracked=@(Get-OrdinalUnique @($ordinary+$ignored))
    [pscustomobject]@{Tracked=@(Get-OrdinalUnique $tracked);Untracked=@(Get-OrdinalUnique $untracked);All=@(Get-OrdinalUnique (@($tracked)+@($untracked)))}
}
function Remove-WorktreeChanges([string]$Worktree,[object[]]$Allowed=@(),[object[]]$Forbidden=@(),[switch]$All) {
    $changes=Get-WorktreeChanges $Worktree; $targets=@($changes.All|Where-Object{$All -or -not(Test-AllowedPath $_ $Allowed $Forbidden)})
    foreach($path in $targets){
        $full=Get-CanonicalPath (Join-Path $Worktree $path)
        if(-not(Test-Contained $full $Worktree)){throw 'refusing to clean a path outside worktree'}
        if($env:FICANT_TEST_CLEANUP_FAIL_PATH -and $path -eq $env:FICANT_TEST_CLEANUP_FAIL_PATH){throw "injected cleanup failure: $path"}
        $reparse=Get-ReparsePathComponent $full $Worktree
        if($reparse){Remove-SafeWorktreeNode $reparse $Worktree;continue}
        if($changes.Untracked -contains $path){if(Test-Path -LiteralPath $full){Remove-SafeWorktreeNode $full $Worktree}}
        else{
            $inHead=Invoke-ArgvProcess 'git' @('-C',$Worktree,'cat-file','-e',"HEAD:$path") '' 60
            if($inHead.ExitCode -eq 0){$out=Invoke-ArgvProcess 'git' @('-C',$Worktree,'restore','--source=HEAD','--staged','--worktree','--',":(literal)$path") '' 60}
            else{$out=Invoke-ArgvProcess 'git' @('-C',$Worktree,'restore','--staged','--',":(literal)$path") '' 60;if($out.ExitCode -eq 0 -and (Test-Path -LiteralPath $full)){Remove-SafeWorktreeNode $full $Worktree}}
            if($out.ExitCode -ne 0){throw "could not restore unauthorized change '$path': $($out.Error.Trim())"}
        }
    }
    $remaining=Get-WorktreeChanges $Worktree
    $bad=@($remaining.All|Where-Object{$All -or -not(Test-AllowedPath $_ $Allowed $Forbidden)})
    if($bad.Count){throw "worktree cleanup verification failed: $($bad -join ', ')"}
    [pscustomobject]@{Attempted=$targets.Count -gt 0;Removed=$targets}
}
function Assert-PhaseBoundary([string]$Worktree,[bool]$ReadOnly,[object[]]$Allowed,[object[]]$Forbidden,[string]$Phase) {
    $changes=Get-WorktreeChanges $Worktree
    try{Assert-CandidatePathsPhysical $Worktree $changes}catch{$physical=$_;try{Remove-WorktreeChanges $Worktree @() -All|Out-Null}catch{throw "$Phase created a reparse candidate path; cleanup failure: $($_.Exception.Message)"};throw "$Phase created a reparse candidate path; cleanup verified: $($physical.Exception.Message)"}
    if(-not $changes.All.Count){return [pscustomobject]@{Attempted=$false;Removed=@()}}
    if($ReadOnly){try{$clean=Remove-WorktreeChanges $Worktree @() -All}catch{throw "$Phase mutated a read-only worktree; cleanup failure: $($_.Exception.Message)"};throw "$Phase mutated a read-only worktree; cleanup verified"}
    $unauthorized=@($changes.All|Where-Object{-not(Test-AllowedPath $_ $Allowed $Forbidden)})
    if($unauthorized.Count){try{$clean=Remove-WorktreeChanges $Worktree $Allowed $Forbidden}catch{throw "$Phase created unauthorized or forbidden changes: $($unauthorized -join ', '); cleanup failure: $($_.Exception.Message)"};throw "$Phase created unauthorized or forbidden changes: $($unauthorized -join ', '); cleanup verified"}
    [pscustomobject]@{Attempted=$false;Removed=@()}
}
function Invoke-Run([string]$ContractPath,[string]$TestDoublePath,[string]$Root) {
    $started=[System.Diagnostics.Stopwatch]::StartNew()
    Test-ExecutionConfig $Root|Out-Null; Test-Contract $ContractPath $Root|Out-Null; $c=Read-JsonObject $ContractPath
    if((Get-WorktreeChanges $c.worktree).All.Count){throw 'worktree must start clean'}
    $readOnlyPermission=$c.permission_profile -in @('test-executor','quality','audit')
    $temporary=$null
    if(-not(Test-Path -LiteralPath $c.evidence_path)){New-Item -ItemType Directory -Path $c.evidence_path|Out-Null};[IO.File]::WriteAllText((Join-Path $c.evidence_path '.ficant-runner-owned'),[string]$c.task_id)
    try {
    $pre=Invoke-Preflight $c.profile $c.permission_profile $(if($TestDoublePath -and (Test-Path $TestDoublePath -PathType Leaf)){Split-Path $TestDoublePath}else{$TestDoublePath})|ConvertFrom-Json
    $capability=Assert-CapabilityEvidence $c $root
    $sandbox=if($readOnlyPermission){'read-only'}else{'workspace-write'}
    $commands=@();$passed=0;$failed=0
    if($readOnlyPermission) {
        foreach($kind in @('green','regression')){foreach($command in $c.commands.$kind){$run=Invoke-ContractCommand $command $kind $true;$commands+=$run.Evidence;if($run.Outcome.ExitCode -ne 0 -or ($command.expected_tests -gt 0 -and $run.Evidence.observed_tests -ne $command.expected_tests)){$failed++;try{Assert-PhaseBoundary $c.worktree $true $c.allowed_paths $c.forbidden_paths "command phase $kind"|Out-Null}catch{throw};throw "$kind evidence failed"}else{$passed += $run.Evidence.observed_tests}};Assert-PhaseBoundary $c.worktree $true $c.allowed_paths $c.forbidden_paths "command phase $kind"|Out-Null}
    }
    $workerForbidden=@($c.forbidden_paths|Where-Object{[string]$_ -notmatch '^[A-Za-z]:\\'})
    $prompt="Task $($c.task_id)`nObjective: $($c.objective)`nallowed_paths: $($c.allowed_paths -join ', ')`nforbidden_paths: $($workerForbidden -join ', ')`n$($c.instructions -join "`n")"
    if($readOnlyPermission){
        $briefEvidence=$commands|ConvertTo-Json -Depth 10 -Compress
        $fence=([string][char]96)*3
        $prompt += "`nRunner-managed tests completed before this brief. Organize this structured command evidence; do not write files.`n${fence}json`n$briefEvidence`n$fence"
    }
    if($TestDoublePath -and (Test-Path $TestDoublePath -PathType Leaf)){ $worker=Invoke-ArgvProcess 'pwsh' @('-NoProfile','-File',$TestDoublePath,$c.worktree) $prompt $c.timeout_seconds $c.worktree }
    elseif($c.profile -eq 'medium'){ $worker=Invoke-ArgvProcess 'claude' @('-p','--model',$c.model.requested,'--permission-mode',$(if($sandbox -eq 'read-only'){'plan'}else{'acceptEdits'}),'--output-format','json','--no-session-persistence') $prompt $c.timeout_seconds $c.worktree }
    else{$worker=Invoke-ArgvProcess 'codex' @('exec','--ephemeral','--ignore-user-config','-c','windows.sandbox="elevated"','--model',$c.model.requested,'--sandbox',$sandbox,'--json','-C',$c.worktree,'-') $prompt $c.timeout_seconds}
    if((Get-Git $c.worktree @('rev-parse','HEAD')) -ne $c.base_sha){throw 'worker changed HEAD/base'}
    $workerActual=if($c.profile -eq 'medium'){Get-ClaudeModelIdentity ($worker.Output|ConvertFrom-Json)}else{[string]$c.model.requested}
    $workerIdentitySource=if($c.profile -eq 'medium'){'provider-reported-actual'}else{'explicit-cli-selection'}
    [IO.File]::WriteAllText((Join-Path $c.evidence_path 'raw-output.txt'),$worker.Output+[Environment]::NewLine+$worker.Error)
    try { Assert-PhaseBoundary $c.worktree $readOnlyPermission $c.allowed_paths $c.forbidden_paths 'model execution'|Out-Null }
    catch { $original=$_; try { if((Get-WorktreeChanges $c.worktree).All.Count){$cleanupAllowed=if($readOnlyPermission){@()}else{@($c.allowed_paths)};Remove-WorktreeChanges $c.worktree $cleanupAllowed $c.forbidden_paths -All:$readOnlyPermission|Out-Null} } catch { throw "$($original.Exception.Message); cleanup failure: $($_.Exception.Message)" }; throw $original }
    if($worker.ExitCode -ne 0){throw 'worker execution failed'}
    $changed=@((Get-WorktreeChanges $c.worktree).All|ForEach-Object{$_.Replace('\','/')})
    if(-not $readOnlyPermission) {
        foreach($kind in @('green','regression')){foreach($command in $c.commands.$kind){$run=Invoke-ContractCommand $command $kind $false;$commands+=$run.Evidence;if($run.Outcome.ExitCode -ne 0 -or ($command.expected_tests -gt 0 -and $run.Evidence.observed_tests -ne $command.expected_tests)){$failed++;Assert-PhaseBoundary $c.worktree $false $c.allowed_paths $c.forbidden_paths "command phase $kind"|Out-Null;throw "$kind evidence failed"}else{$passed += $run.Evidence.observed_tests}};Assert-PhaseBoundary $c.worktree $false $c.allowed_paths $c.forbidden_paths "command phase $kind"|Out-Null}
        $changed=@((Get-WorktreeChanges $c.worktree).All|ForEach-Object{$_.Replace('\','/')})
    }
    Assert-PhaseBoundary $c.worktree $readOnlyPermission $c.allowed_paths $c.forbidden_paths 'final candidate' | Out-Null
    $diff=Get-Git $c.worktree @('diff','--binary'); $diffHash=(Get-Sha256Text $diff).Substring(7); $raw=Join-Path $c.evidence_path 'raw-output.txt'; $commandPath=Join-Path $c.evidence_path 'commands.json'; [IO.File]::WriteAllText($commandPath,($commands|ConvertTo-Json -Depth 10))
    $candidate=Get-CandidateManifest $c.worktree
    $manifestPath=Join-Path $c.evidence_path 'candidate-manifest.json'
    [IO.File]::WriteAllText($manifestPath,$candidate.Json,[Text.UTF8Encoding]::new($false))
    $validationStarted=[Diagnostics.Stopwatch]::StartNew()
    $result=[ordered]@{schema_version=5;status='ready';checklist_id=$c.checklist_id;task_id=$c.task_id;profile=$c.profile;executor=$(if($c.profile -eq 'medium'){'claude'}else{'codex'});command_executor=$(if($readOnlyPermission){'runner-managed'}else{'worker-direct'});requested_model=$c.model.requested;actual_model=$workerActual;model_identity_source=$workerIdentitySource;permission_profile=$c.permission_profile;requested_sandbox=$sandbox;effective_sandbox=$pre.effective_sandbox;environment='windows';capability_evidence_id=$c.capability_evidence_id;capability_evidence_path=$capability.Path;base_sha=$c.base_sha;candidate_sha=(Get-Git $c.worktree @('rev-parse','HEAD'));candidate_state='verified-diff';candidate_diff_sha256=$candidate.Sha256;changed_files=$changed;commands=$commands;tests=[ordered]@{passed=$passed;failed=$failed;skipped=0;total=$passed+$failed};evidence_path=$c.evidence_path;evidence=@([ordered]@{path=$raw;sha256=(Get-FileHash $raw -Algorithm SHA256).Hash.ToLowerInvariant()},[ordered]@{path=$commandPath;sha256=(Get-FileHash $commandPath -Algorithm SHA256).Hash.ToLowerInvariant()},[ordered]@{path=$manifestPath;sha256=(Get-FileHash $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant()},[ordered]@{path=$capability.Path;sha256=(Get-FileHash $capability.Path -Algorithm SHA256).Hash.ToLowerInvariant()});recovery=[ordered]@{correction_budget=$c.recovery_policy.max_correction_cycles;correction_cycles_used=0;events=@()};escalated=$false;escalation_reason=$null;blockers=@();summary='runner-verified bounded candidate';timings=[ordered]@{model_execution_ms=$worker.DurationMs;runner_validation_ms=1;total_ms=1};cleanup=[ordered]@{temporary_paths_removed=$true;worktree_state_recorded=$true}}
    $temporary=Join-Path $c.evidence_path ('.result-'+[guid]::NewGuid().ToString('n')+'.tmp')
    [IO.File]::WriteAllText($temporary,($result|ConvertTo-Json -Depth 20))
    Test-Result $temporary $Root|Out-Null
    $result.timings.runner_validation_ms=[Math]::Max(1,[int]$validationStarted.ElapsedMilliseconds)
    $result.timings.total_ms=[Math]::Max(1,[int]$started.ElapsedMilliseconds)
    [IO.File]::WriteAllText($temporary,($result|ConvertTo-Json -Depth 20))
    Test-Result $temporary $Root|Out-Null
    Move-Item -LiteralPath $temporary -Destination $c.result_path -Force
    Test-Result $c.result_path $Root|Out-Null
    $validationStarted.Stop()
    $started.Stop()
    $result.timings.runner_validation_ms=[Math]::Max(1,[int]$validationStarted.ElapsedMilliseconds)
    $result.timings.total_ms=[Math]::Max(1,[int]$started.ElapsedMilliseconds)
    [IO.File]::WriteAllText($c.result_path,($result|ConvertTo-Json -Depth 20))
    $result|ConvertTo-Json -Depth 20 -Compress
    } catch {
        $original=$_
        $cleanupError=$null
        $currentHead=$null
        try{$currentHead=Get-Git $c.worktree @('rev-parse','HEAD')}catch{}
        if($currentHead -and $currentHead -ne $c.base_sha){
            try{
                if($temporary -and (Test-Path -LiteralPath $temporary)){Remove-Item -LiteralPath $temporary -Force -ErrorAction Stop}
                if(Test-Path -LiteralPath $c.result_path){Remove-Item -LiteralPath $c.result_path -Force -ErrorAction Stop}
            }catch{throw "cleanup failure: $($_.Exception.Message); HEAD/base drift requires disposal of the isolated worktree"}
            throw 'cleanup failure: HEAD/base drift requires disposal of the isolated worktree'
        }
        try {
            if($temporary -and (Test-Path -LiteralPath $temporary)){Remove-Item -LiteralPath $temporary -Force -ErrorAction Stop}
            if(Test-Path -LiteralPath $c.result_path){Remove-Item -LiteralPath $c.result_path -Force -ErrorAction Stop}
            $cleanupAllowed=if($readOnlyPermission){@()}else{@($c.allowed_paths)}
            Remove-WorktreeChanges $c.worktree $cleanupAllowed $c.forbidden_paths -All:$readOnlyPermission|Out-Null
        } catch {$cleanupError=$_}
        if($cleanupError){throw "$($original.Exception.Message); cleanup failure: $($cleanupError.Exception.Message)"}
        throw $original
    }
}
function Invoke-FixedIncomeWave1([string]$TaskId,[string]$BaseSha,[string]$Worktree,[string]$CapabilityEvidencePath,[string]$Root,[string]$Profile,[string]$Permission) {
    if($TaskId -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'){throw 'TaskId must be a safe runner evidence identifier'}
    if($BaseSha -notmatch '^[0-9a-f]{40}$'){throw 'BaseSha must be an exact lowercase Git SHA'}
    if($Profile -ne 'fast' -or $Permission -ne 'test-executor'){throw 'FixedIncomeWave1 requires fast/test-executor capability context'}
    Assert-WindowsAbsolutePath $Worktree 'Worktree'
    Assert-WindowsAbsolutePath $CapabilityEvidencePath 'CapabilityEvidencePath'
    $worktreePath=Get-CanonicalPath (Resolve-Path -LiteralPath $Worktree).Path
    Assert-LinkedWorktree $worktreePath
    $head=Get-Git $worktreePath @('rev-parse','HEAD')
    if($head -ne $BaseSha){throw 'worktree HEAD does not match exact BaseSha'}
    if((Get-WorktreeChanges $worktreePath).All.Count){throw 'FixedIncomeWave1 requires a clean read-only source worktree'}

    $capabilityDocument=Read-JsonObject $CapabilityEvidencePath
    $capabilityContract=[pscustomobject]@{
        capability_evidence_path=$CapabilityEvidencePath
        capability_evidence_id=[string]$capabilityDocument.capability_evidence_id
        profile=$Profile
        permission_profile=$Permission
    }
    $capability=Assert-CapabilityEvidence $capabilityContract $Root
    $requiredTools=@('cmake','ctest','ninja','vs-clang','vs-clang++','vs-clang-cl','vs-llvm-rc')
    $requested=@($capability.Evidence.requested_capabilities)
    if($requested.Count -ne $requiredTools.Count -or @($requiredTools|Where-Object{$_ -notin $requested}).Count){throw 'FixedIncomeWave1 capability evidence must contain exactly cmake, ctest, ninja, vs-clang, vs-clang++, vs-clang-cl, and vs-llvm-rc'}

    $evidenceRoot=Get-EvidenceRoot
    $evidencePath=Get-CanonicalPath (Join-Path $evidenceRoot $TaskId)
    if(-not((Split-Path $evidencePath -Parent).Equals($evidenceRoot,[StringComparison]::OrdinalIgnoreCase))){throw 'task evidence path escaped the runner evidence root'}
    if(Test-Path -LiteralPath $evidencePath){throw 'task evidence path already exists'}
    if(Test-Contained $evidenceRoot $worktreePath -or Test-Contained $worktreePath $evidenceRoot){throw 'evidence root must be outside the source worktree'}
    Assert-NoReparsePathComponents $evidencePath $evidenceRoot
    [IO.Directory]::CreateDirectory($evidencePath)|Out-Null
    [IO.File]::WriteAllText((Join-Path $evidencePath '.ficant-runner-owned'),$TaskId,[Text.UTF8Encoding]::new($false))
    $resultPath=Join-Path $evidencePath 'result.json'

    $scratchConfigured=[Environment]::GetEnvironmentVariable('FICANT_RUNNER_SCRATCH_ROOT')
    if(-not $scratchConfigured){$scratchConfigured=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'ficant\runner-scratch'}
    Assert-WindowsAbsolutePath $scratchConfigured 'FICANT_RUNNER_SCRATCH_ROOT'
    $scratchRoot=Get-CanonicalPath $scratchConfigured
    $scratchPath=Get-CanonicalPath (Join-Path $scratchRoot $TaskId)
    if(-not((Split-Path $scratchPath -Parent).Equals($scratchRoot,[StringComparison]::OrdinalIgnoreCase))){throw 'task scratch path escaped the runner scratch root'}
    foreach($record in (Split-Nul (Get-Git $worktreePath @('worktree','list','--porcelain','-z')))){
        if($record.StartsWith('worktree ')){
            $registered=Get-CanonicalPath $record.Substring(9)
            if(Test-Contained $scratchRoot $registered -or Test-Contained $registered $scratchRoot){throw 'runner scratch root must be outside every registered worktree'}
        }
    }
    if(Test-Path -LiteralPath $scratchPath){throw 'task scratch path already exists'}
    Assert-NoReparsePathComponents $scratchPath $scratchRoot
    [IO.Directory]::CreateDirectory($scratchPath)|Out-Null
    $buildPath=Join-Path $scratchPath 'build'
    $sourcePath=Join-Path $worktreePath 'cpp\fixed-income-kernel'
    if(-not(Test-Path -LiteralPath (Join-Path $sourcePath 'CMakeLists.txt') -PathType Leaf)){throw 'fixed-income kernel source is missing'}

    $tools=$capability.Evidence.tools
    $cmake=[string]$tools.cmake.path
    $ctest=[string]$tools.ctest.path
    $clangxx=[string]$tools.'vs-clang++'.path
    $llvmrc=[string]$tools.'vs-llvm-rc'.path
    $ninja=[string]$tools.ninja.path
    $configureArgv=@($cmake,'-S',$sourcePath,'-B',$buildPath,'-G','Ninja',"-DCMAKE_CXX_COMPILER:FILEPATH=$clangxx","-DCMAKE_RC_COMPILER:FILEPATH=$llvmrc","-DCMAKE_MAKE_PROGRAM:FILEPATH=$ninja",'-DCMAKE_BUILD_TYPE=Debug','-DBUILD_TESTING=ON')
    $buildArgv=@($cmake,'--build',$buildPath,'--parallel','2')
    $testArgv=@($ctest,'--test-dir',$buildPath,'--output-on-failure','--no-tests=error')
    $commands=@()
    $scratchRemoved=$false
    try {
        $specifications=@(
            [pscustomobject]@{Identity='fixed-income-wave1-configure';Argv=$configureArgv;Expected=0;Evidence='configure.txt';Timeout=180},
            [pscustomobject]@{Identity='fixed-income-wave1-build';Argv=$buildArgv;Expected=0;Evidence='build.txt';Timeout=300},
            [pscustomobject]@{Identity='fixed-income-wave1-ctest';Argv=$testArgv;Expected=4;Evidence='ctest.txt';Timeout=180}
        )
        foreach($specification in $specifications){
            $argv=@($specification.Argv)
            $outcome=Invoke-ArgvProcess $argv[0] @($argv|Select-Object -Skip 1) '' $specification.Timeout $worktreePath
            $commandEvidencePath=Join-Path $evidencePath $specification.Evidence
            $combined="stdout:`r`n$($outcome.Output)`r`nstderr:`r`n$($outcome.Error)"
            [IO.File]::WriteAllText($commandEvidencePath,$combined,[Text.UTF8Encoding]::new($false))
            $observed=0
            if($specification.Expected -gt 0 -and $combined -match '(?m)out of\s+(\d+)'){$observed=[int]$Matches[1]}
            $commands += [ordered]@{catalog_identity=$specification.Identity;argv=$argv;cwd=$worktreePath;exit_code=$outcome.ExitCode;duration_ms=$outcome.DurationMs;expected_tests=$specification.Expected;observed_tests=$observed;evidence_path=$commandEvidencePath}
            if($outcome.ExitCode -ne 0){throw "$($specification.Identity) failed with exit code $($outcome.ExitCode)"}
            if($specification.Expected -gt 0 -and $observed -ne $specification.Expected){throw "$($specification.Identity) observed $observed tests; expected $($specification.Expected)"}
        }
        $ctestText=[IO.File]::ReadAllText((Join-Path $evidencePath 'ctest.txt'))
        if($ctestText -notmatch '(?m)100% tests passed, 0 tests failed out of 4'){throw 'CTest did not report the frozen 4/4 success summary'}
        if((Get-Git $worktreePath @('rev-parse','HEAD')) -ne $BaseSha){throw 'source HEAD changed during runner-owned test execution'}
        if((Get-WorktreeChanges $worktreePath).All.Count){throw 'runner-owned test execution mutated the read-only source worktree'}
    } finally {
        if(Test-Path -LiteralPath $scratchPath){Remove-SafeWorktreeNode $scratchPath $scratchRoot}
        $scratchRemoved=-not(Test-Path -LiteralPath $scratchPath)
        if(-not $scratchRemoved){throw 'runner scratch cleanup verification failed'}
    }

    $candidateSha=Get-Git $worktreePath @('rev-parse','HEAD')
    $evidence=@()
    foreach($path in @((Join-Path $evidencePath 'configure.txt'),(Join-Path $evidencePath 'build.txt'),(Join-Path $evidencePath 'ctest.txt'),$capability.Path)){
        $evidence += [ordered]@{path=$path;sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()}
    }
    $result=[ordered]@{
        schema_version=1;status='ready';action='fixed-income-wave1';task_id=$TaskId;environment='windows';permission_profile='test-executor';command_executor='runner-managed';model_invoked=$false
        worktree=$worktreePath;base_sha=$BaseSha;candidate_sha=$candidateSha;capability_evidence_id=[string]$capability.Evidence.capability_evidence_id;capability_evidence_path=$capability.Path
        tools=[ordered]@{cmake=$tools.cmake;ctest=$tools.ctest;ninja=$tools.ninja;'vs-clang'=$tools.'vs-clang';'vs-clang++'=$tools.'vs-clang++';'vs-clang-cl'=$tools.'vs-clang-cl';'vs-llvm-rc'=$tools.'vs-llvm-rc'}
        commands=$commands;tests=[ordered]@{passed=4;failed=0;skipped=0;total=4};evidence_path=$evidencePath;evidence=$evidence
        cleanup=[ordered]@{scratch_path=$scratchPath;scratch_removed=$scratchRemoved;source_clean_before=$true;source_clean_after=$true}
    }
    $temporary=Join-Path $evidencePath ('.result-'+[guid]::NewGuid().ToString('n')+'.tmp')
    try {
        [IO.File]::WriteAllText($temporary,($result|ConvertTo-Json -Depth 20),[Text.UTF8Encoding]::new($false))
        Invoke-Validator $Root @('instance','--schema',(Join-Path $Root 'schemas\fixed-income-wave1-result.schema.json'),'--instance',$temporary)
        Move-Item -LiteralPath $temporary -Destination $resultPath
    } finally {if(Test-Path -LiteralPath $temporary){Remove-Item -LiteralPath $temporary -Force}}
    $result|ConvertTo-Json -Depth 20 -Compress
}
function Invoke-WindowsWorkerAction { param([string]$Action,[string]$Contract,[string]$ResultPath,[string]$RequestedCapabilities,[string]$Profile,[string]$Permission,[string]$ConfigRoot,[string]$TaskId,[string]$BaseSha,[string]$Worktree,[string]$CapabilityEvidencePath)
    $root=if($ConfigRoot){$ConfigRoot}else{$PSScriptRoot}; switch($Action){'ValidateConfig'{Test-ExecutionConfig $root};'CapabilityPreflight'{Get-CapabilityEvidence $RequestedCapabilities $root $Profile $Permission $ResultPath};'ValidateContract'{Test-Contract $Contract $root};'ValidateResult'{Test-Result $ResultPath $root};'Preflight'{Invoke-Preflight $Profile $Permission ''};'Run'{Invoke-Run $Contract '' $root};'FixedIncomeWave1'{Invoke-FixedIncomeWave1 $TaskId $BaseSha $Worktree $CapabilityEvidencePath $root $Profile $Permission}}
}
function Invoke-WindowsWorkerTestAction { param([string]$Action,[string]$Contract,[string]$ResultPath,[string]$RequestedCapabilities,[string]$Profile,[string]$Permission,[string]$TestDoublePath,[string]$ConfigRoot)
    $root=if($ConfigRoot){$ConfigRoot}else{$PSScriptRoot}; switch($Action){'Preflight'{Invoke-Preflight $Profile $Permission $TestDoublePath};'Run'{Invoke-Run $Contract $TestDoublePath $root};default{Invoke-WindowsWorkerAction -Action $Action -Contract $Contract -ResultPath $ResultPath -RequestedCapabilities $RequestedCapabilities -Profile $Profile -Permission $Permission -ConfigRoot $ConfigRoot}}
}
