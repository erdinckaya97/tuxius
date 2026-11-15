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
    Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/$repo/releases/latest/download/tuxius-$target.zip" -OutFile $zip
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
