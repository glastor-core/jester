<#
.SYNOPSIS
  One-line installer for GLASTOR zerho on Windows x86_64 with SHA-256 verification.
.EXAMPLE
  irm https://raw.githubusercontent.com/glastor-core/jester/main/scripts/install.ps1 | iex
#>
$ErrorActionPreference = "Stop"

$Repo = "glastor-core/jester"
$Archive = "zerho-windows-x86_64.zip"
$BaseUrl = "https://github.com/$Repo/releases/latest/download"
$InstallDir = if ($env:ZERHO_INSTALL_DIR) { $env:ZERHO_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "zerho\bin" }

$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("zerho-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null

try {
    $ZipPath = Join-Path $TmpDir $Archive
    $ShaPath = Join-Path $TmpDir "$Archive.sha256"

    Write-Host "Downloading $Archive from $Repo..."
    Invoke-WebRequest -Uri "$BaseUrl/$Archive" -OutFile $ZipPath -UseBasicParsing
    Invoke-WebRequest -Uri "$BaseUrl/$Archive.sha256" -OutFile $ShaPath -UseBasicParsing

    $ExpectedHash = ((Get-Content $ShaPath -Raw).Trim() -split "\s+")[0].ToLowerInvariant()
    $ActualHash = (Get-FileHash -Path $ZipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ExpectedHash -ne $ActualHash) {
        throw "SHA-256 mismatch: expected $ExpectedHash, got $ActualHash"
    }
    Write-Host "SHA-256 verified ($ActualHash)."

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Expand-Archive -Path $ZipPath -DestinationPath $InstallDir -Force

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $UserPath -or ($UserPath -split ";") -notcontains $InstallDir) {
        $NewPath = if ($UserPath) { "$UserPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        Write-Host "Added $InstallDir to User PATH (restart terminal to pick up)."
    }

    & (Join-Path $InstallDir "zerho.exe") --version
} finally {
    Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue
}
