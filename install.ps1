# Tuxius installer for Windows.
#   irm https://raw.githubusercontent.com/erdinckaya97/tuxius/main/install.ps1 | iex
# Set TUXIUS_INSTALL_DIR to choose where tuxius.exe goes.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$repo = 'erdinckaya97/tuxius'
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$target = "$arch-pc-windows-msvc"
$dir = if ($env:TUXIUS_INSTALL_DIR) { $env:TUXIUS_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\tuxius' }

Write-Host "> Installing Tuxius ($target)" -ForegroundColor Yellow

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("tuxius-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $zip = Join-Path $tmp 'tuxius.zip'
    # /releases/latest/download keeps redirecting to the previous file after a
    # release. Ask the API for the tag, then download that exact archive.
    $headers = @{
        'User-Agent'      = 'tuxius'
        'Accept'          = 'application/vnd.github+json'
        'Cache-Control'   = 'no-cache'
        'Pragma'          = 'no-cache'
    }
    $release = Invoke-RestMethod -UseBasicParsing -Headers $headers -Uri "https://api.github.com/repos/$repo/releases/latest"
    $tag = $release.tag_name
    if (-not $tag) { throw "could not find the latest Tuxius release" }
    Invoke-WebRequest -UseBasicParsing -Headers @{ 'Cache-Control' = 'no-cache'; 'Pragma' = 'no-cache' } -Uri "https://github.com/$repo/releases/download/$tag/tuxius-$target.zip" -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Copy-Item -Path (Join-Path $tmp 'tuxius.exe') -Destination (Join-Path $dir 'tuxius.exe') -Force
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not $userPath) { $userPath = '' }
if (-not (($userPath -split ';') -contains $dir)) {
    $newPath = (($userPath.TrimEnd(';'), $dir) | Where-Object { $_ }) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    $env:Path = "$env:Path;$dir"
    Write-Host "  Added $dir to your PATH. Open a new terminal to use it everywhere."
}

if (-not (Get-Command ssh -ErrorAction SilentlyContinue)) {
    Write-Host "! OpenSSH client not found. Install it from Settings > System > Optional features > OpenSSH Client." -ForegroundColor Yellow
}

Write-Host "Tuxius installed to $dir\tuxius.exe. Type 'tuxius' to start." -ForegroundColor Green
