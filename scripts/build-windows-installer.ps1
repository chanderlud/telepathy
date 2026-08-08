$ErrorActionPreference = 'Stop'

if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw 'Windows is required to build the Windows installer. Run this script in Windows PowerShell or PowerShell on Windows.'
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$installerScript = Join-Path $repoRoot 'windows\installer.iss'
$releaseBundle = Join-Path $repoRoot 'build\windows\x64\runner\Release'
$installerOutput = Join-Path $repoRoot 'windows\Output\telepathy_installer.exe'
$isccPath = 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'

if (-not (Test-Path -LiteralPath $installerScript -PathType Leaf)) {
    throw "Inno Setup script not found: $installerScript"
}

$flutter = Get-Command flutter -ErrorAction SilentlyContinue
if ($null -eq $flutter) {
    throw 'Flutter CLI not found on PATH. Install Flutter separately, then run this script again.'
}

if (-not (Test-Path -LiteralPath $isccPath -PathType Leaf)) {
    throw "Inno Setup compiler not found at '$isccPath'. Install Inno Setup 6 separately, then run this script again."
}

$rustup = Get-Command rustup -ErrorAction SilentlyContinue
if ($null -eq $rustup) {
    throw 'rustup not found on PATH. Install Rust with rustup separately, then run this script again.'
}

$null = & $rustup.Path run stable rustc --version
if ($LASTEXITCODE -ne 0) {
    throw 'Stable Rust toolchain not found. Install it with: rustup toolchain install stable'
}

$stableTargets = & $rustup.Path target list --toolchain stable --installed
if ($LASTEXITCODE -ne 0) {
    throw "Unable to list stable Rust targets with rustup. Install stable with: rustup toolchain install stable"
}
if ($stableTargets -notcontains 'x86_64-pc-windows-msvc') {
    throw 'Rust target x86_64-pc-windows-msvc is not installed for stable. Install it with: rustup target add --toolchain stable x86_64-pc-windows-msvc'
}

Write-Host "Building Windows release from $repoRoot"
Push-Location $repoRoot
try {
    & $flutter.Path pub get
    if ($LASTEXITCODE -ne 0) {
        throw "Flutter package retrieval failed with exit code $LASTEXITCODE."
    }

    & $flutter.Path build windows --release
    if ($LASTEXITCODE -ne 0) {
        throw "Flutter Windows release build failed with exit code $LASTEXITCODE."
    }

    $releaseExecutable = Join-Path $releaseBundle 'telepathy.exe'
    if (-not (Test-Path -LiteralPath $releaseBundle -PathType Container)) {
        throw "Flutter release bundle not found after build: $releaseBundle"
    }
    if (-not (Test-Path -LiteralPath $releaseExecutable -PathType Leaf)) {
        throw "Flutter release executable not found in bundle: $releaseExecutable"
    }

    Write-Host "Compiling installer from $installerScript"
    & $isccPath $installerScript
    if ($LASTEXITCODE -ne 0) {
        throw "Inno Setup compilation failed with exit code $LASTEXITCODE."
    }
}
finally {
    Pop-Location
}

if (-not (Test-Path -LiteralPath $installerOutput -PathType Leaf)) {
    throw "Installer output not found after compilation: $installerOutput"
}

$resolvedOutput = (Resolve-Path -LiteralPath $installerOutput).Path
Write-Host "Windows installer created: $resolvedOutput"
