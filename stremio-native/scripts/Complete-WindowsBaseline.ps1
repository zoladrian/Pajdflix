[CmdletBinding()]
param([switch]$InstallMissingLLVM, [switch]$AllowBuildChanges)

# Prepare and build the original Windows runtime. Never launch the application.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$toolchain = 'nightly-2026-07-31'
$vcpkgRevision = '84bab45d415d22042bd0b9081aea57f362da3f35'
$triplet = 'x64-windows-v3-static-md-release'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script from a regular, non-elevated PowerShell.'
}

function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE. See prepare.log." }
}

function Find-Libclang {
    $candidates = @('C:/Program Files/LLVM/bin/libclang.dll', 'C:/Program Files (x86)/LLVM/bin/libclang.dll')
    if ($env:LIBCLANG_PATH) { $candidates = @((Join-Path $env:LIBCLANG_PATH 'libclang.dll')) + $candidates }
    foreach ($vs in $vsRoots) {
        $candidates += "$vs/VC/Tools/Llvm/x64/bin/libclang.dll"
        $candidates += "$vs/VC/Tools/Llvm/bin/libclang.dll"
    }
    $clang = Get-Command clang.exe -ErrorAction SilentlyContinue
    if ($clang) { $candidates += Join-Path (Split-Path -Parent $clang.Source) 'libclang.dll' }
    return $candidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}

$vsRoots = @('C:/Program Files/Microsoft Visual Studio/2022/Community', 'C:/Program Files/Microsoft Visual Studio/2022/BuildTools', 'C:/Program Files/Microsoft Visual Studio/18/Community')
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
if (Test-Path -LiteralPath $vswhere) {
    # Stale VC files can survive in an installation without a usable C++ workload.
    $cppRoots = @(& $vswhere -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
    if ($LASTEXITCODE -eq 0 -and $cppRoots.Count -gt 0) { $vsRoots = $cppRoots }
}
$logDir = Join-Path $repo 'artifacts/secure-baseline'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
Push-Location $repo
Start-Transcript -Path (Join-Path $logDir 'prepare.log') -Force | Out-Null
try {
    # No user-wide environment or Git configuration is changed.
    $env:PATH = "$env:USERPROFILE/.cargo/bin;" + [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + $env:PATH
    $env:CARGO_HOME = Join-Path $repo 'artifacts/cargo-home'
    $env:VCPKG_DOWNLOADS = Join-Path $repo 'artifacts/vcpkg-downloads'
    # Avoid writing vcpkg's default binary cache outside the checkout.
    $env:VCPKG_BINARY_SOURCES = 'clear;files,' + (Join-Path $repo 'artifacts/vcpkg-binary-cache') + ',readwrite'
    $env:VCPKG_DISABLE_METRICS = '1'

    Write-Host 'Checking network access before downloading build dependencies...'
    Invoke-Checked 'git.exe' @('ls-remote', 'https://github.com/stremio-native/stream-server.git', 'HEAD')
    Invoke-Checked 'rustup.exe' @('toolchain', 'install', $toolchain, '--profile', 'minimal', '--component', 'clippy', '--component', 'rustfmt', '--no-self-update')
    Invoke-Checked 'git.exe' @('submodule', 'update', '--init', 'vendor/stremio-translations')

    $libclang = Find-Libclang
    if (-not $libclang) {
        if (-not $InstallMissingLLVM) { throw 'libclang.dll missing. Re-run with -InstallMissingLLVM or install LLVM and set LIBCLANG_PATH.' }
        Invoke-Checked 'winget.exe' @('install', '--id', 'LLVM.LLVM', '--exact', '--source', 'winget', '--accept-package-agreements', '--accept-source-agreements')
        $libclang = Find-Libclang
        if (-not $libclang) { throw 'LLVM installer finished but libclang.dll was not found. Set LIBCLANG_PATH to its directory and re-run.' }
    }
    $env:LIBCLANG_PATH = Split-Path -Parent $libclang

    $devShell = $vsRoots | ForEach-Object { "$_/Common7/Tools/Launch-VsDevShell.ps1" } | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    if (-not $devShell) { throw 'Visual Studio developer shell not found. Install the VS 2022 C++ workload.' }
    & $devShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
    if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) { throw 'MSVC compiler unavailable after loading the developer shell.' }
    $cmake = $vsRoots | ForEach-Object { "$_/Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin/cmake.exe" } | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    if ($cmake) { $env:PATH = (Split-Path -Parent $cmake) + ';' + $env:PATH }

    $vcpkgDir = Join-Path $repo 'vcpkg'
    if (-not (Test-Path -LiteralPath $vcpkgDir)) {
        Invoke-Checked 'git.exe' @('clone', 'https://github.com/microsoft/vcpkg.git', $vcpkgDir)
    }
    if (-not (Test-Path -LiteralPath (Join-Path $vcpkgDir '.git'))) { throw 'vcpkg directory exists without Git metadata; inspect it before retrying.' }
    $dirty = @(& git.exe -C $vcpkgDir status --porcelain)
    if ($LASTEXITCODE -ne 0 -or $dirty.Count -gt 0) { throw 'vcpkg has unreadable state or local changes; inspect them before retrying.' }
    Invoke-Checked 'git.exe' @('-C', $vcpkgDir, 'checkout', '--detach', $vcpkgRevision)
    $vcpkgExe = Join-Path $vcpkgDir 'vcpkg.exe'
    if (-not (Test-Path -LiteralPath $vcpkgExe)) {
        Invoke-Checked (Join-Path $vcpkgDir 'bootstrap-vcpkg.bat') @('-disableMetrics')
    }
    $env:VCPKG_ROOT = $vcpkgDir
    Invoke-Checked $vcpkgExe @('install', "--x-manifest-root=$repo", "--x-install-root=$repo/vcpkg_installed", "--triplet=$triplet", "--overlay-triplets=$repo/triplets", "--overlay-ports=$repo/vcpkg-overlays")

    if ($AllowBuildChanges) {
        Invoke-Checked 'powershell.exe' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $PSScriptRoot 'Restore-PinnedMpvSdk.ps1'))
    }
    $buildArguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $PSScriptRoot 'Invoke-WindowsBaseline.ps1'), '-Build')
    if ($AllowBuildChanges) { $buildArguments += '-AllowBuildChanges' }
    Invoke-Checked 'powershell.exe' $buildArguments
    Write-Host 'Baseline compiled. Startup, playback and security tests remain separate and have NOT been performed.'
} finally {
    Stop-Transcript | Out-Null
    Pop-Location
}
