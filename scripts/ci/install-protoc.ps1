<#
.SYNOPSIS
    Installs a pinned protoc on a Windows CI runner and puts it on PATH.

.DESCRIPTION
    LanceDB compiles its protobuf definitions at build time, so every Windows
    build needs protoc. It used to come from the Chocolatey community feed,
    which answers 503 at times; `choco install` then reports success with
    nothing installed and the build fails much later with "Could not find
    protoc". This takes the official release from GitHub instead, checks its
    SHA-256, and fails here, loudly, if anything is wrong.
#>
param(
    [string]$Version = '36.2',
    [string]$Sha256 = 'f0c128dc0d8492eceece83bb459a4c0e316764b929ffbf1aa416357fd644edd3'
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

$url = "https://github.com/protocolbuffers/protobuf/releases/download/v$Version/protoc-$Version-win64.zip"
$root = Join-Path $env:RUNNER_TEMP "protoc-$Version"
$zip = "$root.zip"

for ($attempt = 1; ; $attempt++) {
    try {
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
        break
    } catch {
        if ($attempt -ge 4) { throw "Downloading $url failed $attempt times: $($_.Exception.Message)" }
        Write-Host "Download attempt $attempt failed ($($_.Exception.Message)); retrying"
        Start-Sleep -Seconds (5 * $attempt)
    }
}

$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant()
if ($actual -ne $Sha256) { throw "protoc $Version checksum mismatch: expected $Sha256, got $actual" }

Expand-Archive -LiteralPath $zip -DestinationPath $root -Force
$bin = Join-Path $root 'bin'
$exe = Join-Path $bin 'protoc.exe'
$reported = & $exe --version
if ($LASTEXITCODE -ne 0 -or $reported -notmatch [regex]::Escape($Version)) { throw "protoc did not run as expected: $reported" }

Add-Content -LiteralPath $env:GITHUB_PATH -Value $bin
Add-Content -LiteralPath $env:GITHUB_ENV -Value "PROTOC=$exe"
Write-Host "Installed $reported at $exe"
