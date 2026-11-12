# Onyx CLI Universal Installer for Windows (PowerShell)
# Usage:
#   irm https://raw.githubusercontent.com/EMP0RI0M/onyx-cli/main/install.ps1 | iex

$ErrorActionPreference = 'Stop'
$Repo = "EMP0RI0M/onyx-cli"

$Arch = if ([System.Environment]::Is64BitOperatingSystem) {
    if ([System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -eq [System.Runtime.InteropServices.Architecture]::Arm64) {
        "arm64"
    } else {
        "amd64"
    }
} else {
    Write-Error "Unsupported 32-bit Windows architecture."
}

$Asset = "onyx-windows-$Arch.exe"
Write-Host "→ Detected platform: Windows ($Arch)" -ForegroundColor Cyan

# Determine latest release tag
try {
    $ReleaseInfo = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    $Tag = $ReleaseInfo.tag_name
} catch {
    $Tag = "v0.1.0"
}

$Url = "https://github.com/$Repo/releases/download/$Tag/$Asset"
Write-Host "→ Downloading Onyx CLI ($Tag) from: $Url" -ForegroundColor Cyan

$InstallDir = "$env:LOCALAPPDATA\Programs\onyx\bin"
if (!(Test-Path -Path $InstallDir)) {
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
}

$DestPath = "$InstallDir\onyx.exe"
Invoke-WebRequest -Uri $Url -OutFile $DestPath

# Add to user PATH if not present
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
    Write-Host "✓ Added $InstallDir to User PATH." -ForegroundColor Green
}

Write-Host "`n✓ Onyx CLI installed successfully to: $DestPath" -ForegroundColor Green
Write-Host "Restart your terminal and run 'onyx' or 'onyx --help'." -ForegroundColor Yellow
