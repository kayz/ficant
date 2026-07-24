[CmdletBinding()]
param(
    [switch]$ListOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$composeDirectory = Join-Path $repoRoot 'deploy\dev'
$composeFile = Join-Path $composeDirectory 'docker-compose.yml'
$environmentFile = Join-Path $composeDirectory '.env.local'
$requiredEnvironment = @(
    'FICANT_POSTGRES_PASSWORD',
    'FICANT_S3_ACCESS_KEY',
    'FICANT_S3_SECRET_KEY',
    'FICANT_S3_BUCKET',
    'FICANT_PLATFORM_SIGNING_KEY_HEX',
    'FICANT_PLATFORM_TRACE_KEY_HEX',
    'FICANT_EXPERIMENT_CURSOR_KEY_HEX',
    'FICANT_BOOTSTRAP_SUBJECT',
    'FICANT_BOOTSTRAP_BEARER_TOKEN',
    'FICANT_BOOTSTRAP_SCOPES'
)

function New-RandomHex {
    param(
        [Parameter(Mandatory)]
        [ValidateRange(1, 1024)]
        [int]$ByteCount
    )

    $bytes = [byte[]]::new($ByteCount)
    [System.Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
    return [Convert]::ToHexString($bytes).ToLowerInvariant()
}

function Read-LocalEnvironment {
    param(
        [Parameter(Mandatory)]
        [string]$LiteralPath
    )

    $values = @{}
    foreach ($line in Get-Content -LiteralPath $LiteralPath -ErrorAction Stop) {
        if ([string]::IsNullOrWhiteSpace($line) -or $line.TrimStart().StartsWith('#')) {
            continue
        }
        $separator = $line.IndexOf('=')
        if ($separator -le 0) {
            throw "Invalid local environment entry in $LiteralPath."
        }
        $name = $line.Substring(0, $separator)
        $value = $line.Substring($separator + 1)
        if ($values.ContainsKey($name)) {
            throw "Duplicate local environment key '$name' in $LiteralPath."
        }
        $values[$name] = $value
    }
    return $values
}

function Assert-LocalEnvironment {
    param(
        [Parameter(Mandatory)]
        [hashtable]$Values
    )

    foreach ($name in $requiredEnvironment) {
        if (-not $Values.ContainsKey($name) -or [string]::IsNullOrWhiteSpace($Values[$name])) {
            throw "Missing required local environment key '$name'."
        }
    }
    foreach ($name in @(
        'FICANT_PLATFORM_SIGNING_KEY_HEX',
        'FICANT_PLATFORM_TRACE_KEY_HEX',
        'FICANT_EXPERIMENT_CURSOR_KEY_HEX'
    )) {
        if ($Values[$name] -notmatch '^[0-9a-f]{64}$') {
            throw "$name must be exactly 32 bytes encoded as lowercase hexadecimal."
        }
    }
    $scopes = $Values['FICANT_BOOTSTRAP_SCOPES'].Split(
        ',',
        [System.StringSplitOptions]::RemoveEmptyEntries
    )
    foreach ($scope in @('apps:read', 'experiment:read', 'experiment:write')) {
        if ($scopes -notcontains $scope) {
            throw "FICANT_BOOTSTRAP_SCOPES must include $scope."
        }
    }
}

function Invoke-Native {
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,
        [Parameter(Mandatory)]
        [string[]]$ArgumentList
    )

    & $FilePath @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath failed with exit code $LASTEXITCODE."
    }
}

function Get-WorkerAttestation {
    $workerImage = 'ficant/worker:dev'
    $runtimeDigest = (& docker image inspect --format '{{.Id}}' $workerImage)
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to inspect the locally built Worker image '$workerImage'."
    }
    $runtimeDigest = ($runtimeDigest | Select-Object -First 1).Trim()
    if ($runtimeDigest -notmatch '^sha256:[0-9a-f]{64}$') {
        throw "Worker image identity is not a canonical SHA-256 digest: '$runtimeDigest'."
    }

    $sourceArguments = @(
        'run',
        '--rm',
        '--read-only',
        '--cap-drop', 'ALL',
        '--security-opt', 'no-new-privileges:true',
        '--pids-limit', '64',
        '--memory', '128m',
        $workerImage,
        '--print-native-source-digest'
    )
    $sourceDigest = (& docker @sourceArguments)
    if ($LASTEXITCODE -ne 0) {
        throw 'The locally built Worker did not report its embedded native source digest.'
    }
    $sourceDigest = ($sourceDigest | Select-Object -First 1).Trim()
    if ($sourceDigest -notmatch '^sha256:[0-9a-f]{64}$') {
        throw "Worker native source identity is not a canonical SHA-256 digest: '$sourceDigest'."
    }

    return @{
        RuntimeDigest = $runtimeDigest
        SourceDigest = $sourceDigest
    }
}

function Test-GrpcWebSession {
    param(
        [Parameter(Mandatory)]
        [uri]$BaseUri
    )

    $client = [System.Net.Http.HttpClient]::new()
    try {
        $client.Timeout = [TimeSpan]::FromSeconds(20)

        $shellResponse = $client.GetAsync([uri]::new($BaseUri, '/ficant/')).GetAwaiter().GetResult()
        try {
            if (-not $shellResponse.IsSuccessStatusCode) {
                throw "Platform Shell returned HTTP $([int]$shellResponse.StatusCode)."
            }
        } finally {
            $shellResponse.Dispose()
        }

        $request = [System.Net.Http.HttpRequestMessage]::new(
            [System.Net.Http.HttpMethod]::Post,
            [uri]::new($BaseUri, '/ficant-api/ficant.app.v1.PlatformService/GetCurrentSession')
        )
        try {
            $request.Headers.Add('Origin', $BaseUri.GetLeftPart([System.UriPartial]::Authority))
            $request.Headers.Add('X-Grpc-Web', '1')
            $request.Headers.Add('X-User-Agent', 'ficant-dev-up/1')
            $request.Content = [System.Net.Http.ByteArrayContent]::new([byte[]](0, 0, 0, 0, 0))
            $request.Content.Headers.ContentType = [System.Net.Http.Headers.MediaTypeHeaderValue]::new('application/grpc-web+proto')
            $response = $client.SendAsync($request).GetAwaiter().GetResult()
            try {
                if (-not $response.IsSuccessStatusCode) {
                    throw "gRPC-Web session call returned HTTP $([int]$response.StatusCode)."
                }
                $contentType = $response.Content.Headers.ContentType.MediaType
                if ($contentType -notlike 'application/grpc-web*') {
                    throw "gRPC-Web session call returned unexpected content type '$contentType'."
                }
                $payload = $response.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult()
            } finally {
                $response.Dispose()
            }
        } finally {
            $request.Dispose()
        }

        $offset = 0
        $sawSessionMessage = $false
        $sawSuccessTrailer = $false
        while ($offset + 5 -le $payload.Length) {
            $flags = $payload[$offset]
            $length = (
                ([int]$payload[$offset + 1] -shl 24) -bor
                ([int]$payload[$offset + 2] -shl 16) -bor
                ([int]$payload[$offset + 3] -shl 8) -bor
                [int]$payload[$offset + 4]
            )
            $offset += 5
            if ($length -lt 0 -or $offset + $length -gt $payload.Length) {
                throw 'gRPC-Web session response contains a truncated frame.'
            }
            if (($flags -band 0x80) -eq 0) {
                # GetCurrentSessionResponse field 1 is the authenticated Session oneof.
                # Field 2 is SafeError; accepting any protobuf frame would let an
                # unauthenticated response masquerade as a successful UI/API path.
                if ($length -gt 0 -and $payload[$offset] -eq 0x0A) {
                    $sawSessionMessage = $true
                }
            } else {
                $trailer = [System.Text.Encoding]::ASCII.GetString($payload, $offset, $length)
                if ($trailer -match '(?im)^grpc-status:\s*0\s*$') {
                    $sawSuccessTrailer = $true
                }
            }
            $offset += $length
        }
        if ($offset -ne $payload.Length -or -not $sawSessionMessage -or -not $sawSuccessTrailer) {
            throw 'gRPC-Web session response did not contain an authenticated Session and grpc-status 0.'
        }
    } finally {
        $client.Dispose()
    }
}

if ($ListOnly) {
    Write-Output "Generate or reuse ignored local credentials: $environmentFile"
    Write-Output 'Build the complete pinned development topology with temporary fail-closed attestation placeholders.'
    Write-Output 'Derive the Worker runtime image digest and embedded native source digest from ficant/worker:dev.'
    Write-Output "docker compose --project-directory `"$composeDirectory`" --env-file `"$environmentFile`" --file `"$composeFile`" --profile dev --profile ui up --detach --no-build --remove-orphans --wait"
    Write-Output 'Verify the Platform Shell and a real GetCurrentSession gRPC-Web response through /ficant-api.'
    exit 0
}

if (-not (Test-Path -LiteralPath $environmentFile -PathType Leaf)) {
    $entries = @(
        "FICANT_POSTGRES_PASSWORD=$(New-RandomHex -ByteCount 24)",
        "FICANT_S3_ACCESS_KEY=ficant$(New-RandomHex -ByteCount 8)",
        "FICANT_S3_SECRET_KEY=$(New-RandomHex -ByteCount 24)",
        'FICANT_S3_BUCKET=ficant-dev',
        "FICANT_PLATFORM_SIGNING_KEY_HEX=$(New-RandomHex -ByteCount 32)",
        "FICANT_PLATFORM_TRACE_KEY_HEX=$(New-RandomHex -ByteCount 32)",
        "FICANT_EXPERIMENT_CURSOR_KEY_HEX=$(New-RandomHex -ByteCount 32)",
        'FICANT_BOOTSTRAP_SUBJECT=local-platform-user',
        "FICANT_BOOTSTRAP_BEARER_TOKEN=$(New-RandomHex -ByteCount 32)",
        'FICANT_BOOTSTRAP_SCOPES=apps:read,experiment:read,experiment:write'
    )
    [System.IO.File]::WriteAllLines(
        $environmentFile,
        $entries,
        [System.Text.UTF8Encoding]::new($false)
    )
    Write-Output "Created ignored local credential file: $environmentFile"
} else {
    Write-Output "Reusing ignored local credential file: $environmentFile"
}

$localEnvironment = Read-LocalEnvironment -LiteralPath $environmentFile
Assert-LocalEnvironment -Values $localEnvironment

$env:FICANT_WORKER_RUNTIME_IMAGE_DIGEST = "sha256:$('0' * 64)"
$env:FICANT_WORKER_NATIVE_SOURCE_DIGEST = "sha256:$('0' * 64)"
$buildArguments = @(
    'compose',
    '--project-directory', $composeDirectory,
    '--env-file', $environmentFile,
    '--file', $composeFile,
    '--profile', 'dev',
    '--profile', 'ui',
    'build'
)
Invoke-Native -FilePath 'docker' -ArgumentList $buildArguments

$workerAttestation = Get-WorkerAttestation
$env:FICANT_WORKER_RUNTIME_IMAGE_DIGEST = $workerAttestation.RuntimeDigest
$env:FICANT_WORKER_NATIVE_SOURCE_DIGEST = $workerAttestation.SourceDigest

$composeArguments = @(
    'compose',
    '--project-directory', $composeDirectory,
    '--env-file', $environmentFile,
    '--file', $composeFile,
    '--profile', 'dev',
    '--profile', 'ui',
    'up',
    '--detach',
    '--no-build',
    '--remove-orphans',
    '--wait'
)
Invoke-Native -FilePath 'docker' -ArgumentList $composeArguments

$uiPort = if ($localEnvironment.ContainsKey('FICANT_UI_PORT')) {
    [int]$localEnvironment['FICANT_UI_PORT']
} else {
    18083
}
$uiBaseUri = [uri]"http://127.0.0.1:$uiPort"
Test-GrpcWebSession -BaseUri $uiBaseUri
Write-Output "FICANT development environment is ready: $uiBaseUri/ficant/"
