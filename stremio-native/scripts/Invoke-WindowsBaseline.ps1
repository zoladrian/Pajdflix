[CmdletBinding()]
param([switch]$Build, [switch]$AllowBuildChanges)

# Run from a regular, non-elevated x64 developer PowerShell.
# Default: inspect prerequisites. -Build: also build the original runtime sources.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$baseline = '936ee48771bb3d9c242ebc029abe260dae60c617'
$serverRevision = 'f585ab6eda9b1411034548c131bb0dc30c6f5f9e'
$toolchain = 'nightly-2026-07-31'
$triplet = 'x64-windows-v3-static-md-release'
$checks = [System.Collections.Generic.List[object]]::new()
$outputDir = Join-Path $repo 'artifacts/secure-baseline'

function Record-Check([string]$Name, [bool]$Passed, [string]$Detail) {
    $checks.Add([pscustomobject]@{ name = $Name; passed = $Passed; detail = $Detail })
    $label = if ($Passed) { 'OK' } else { 'MISSING' }
    Write-Host "[$label] ${Name}: $Detail"
}

function Find-Tool([string]$Name, [string[]]$Candidates = @()) {
    $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($command) { return $command.Source }
    foreach ($candidate in $Candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
    }
    return $null
}

Push-Location $repo
$savedPath = $env:PATH
$savedLibclang = $env:LIBCLANG_PATH
$savedCargoHome = $env:CARGO_HOME
try {
    New-Item -ItemType Directory -Force -Path $outputDir | Out-Null
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    Record-Check 'Non-elevated process' (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) 'Run the application and this script without administrator privileges.'

    $git = Find-Tool 'git.exe'
    Record-Check 'Git' ([bool]$git) "$git"
    if (-not $git) { throw 'Git is required to verify the baseline.' }
    $head = & $git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read checkout revision.' }
    # Permit later documentation/script commits, but require original runtime/build inputs.
    $runtimePaths = @('app', 'core-env', 'credential-store', 'debrid', 'media-integrations', 'playback-mpv', 'server-connector', 'stream-ranking', 'vendor', '.cargo', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'mpv.lock.json', 'vcpkg.json', 'triplets', 'vcpkg-overlays')
    & $git diff --quiet $baseline -- @runtimePaths
    $originalInputs = $LASTEXITCODE -eq 0
    Record-Check 'Build input scope' ($originalInputs -or $AllowBuildChanges) "Original inputs=$originalInputs; modified build authorized=$AllowBuildChanges; base=$baseline. See git-status.txt."
    & $git status --short | Set-Content -LiteralPath (Join-Path $outputDir 'git-status.txt') -Encoding UTF8
    $manifest = Get-Content -LiteralPath (Join-Path $repo 'Cargo.toml') -Raw
    $lock = Get-Content -LiteralPath (Join-Path $repo 'Cargo.lock') -Raw
    Record-Check 'Pinned Stream Server' ($manifest.Contains($serverRevision) -and $lock.Contains($serverRevision)) $serverRevision
    $submodule = @(& $git submodule status -- vendor/stremio-translations)
    Record-Check 'Translations submodule' ($LASTEXITCODE -eq 0 -and $submodule.Count -eq 1 -and $submodule[0].StartsWith(' ')) 'Requires git submodule update --init vendor/stremio-translations at the recorded revision.'

    $cargoCandidates = @()
    if ($env:CARGO_HOME) { $cargoCandidates += Join-Path $env:CARGO_HOME 'bin/cargo.exe' }
    if ($env:USERPROFILE) { $cargoCandidates += Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
    $cargo = Find-Tool 'cargo.exe' $cargoCandidates
    Record-Check 'Cargo' ([bool]$cargo) "$cargo"
    $rustupCandidates = @()
    if ($cargo) { $rustupCandidates += Join-Path (Split-Path -Parent $cargo) 'rustup.exe' }
    $rustup = Find-Tool 'rustup.exe' $rustupCandidates
    $installed = if ($rustup) { @(& $rustup toolchain list) } else { @() }
    Record-Check 'Pinned Rust toolchain' ([bool]($installed | Where-Object { $_ -match "^$toolchain-x86_64-pc-windows-msvc(?:\s|$)" })) "Install rustup toolchain $toolchain with clippy and rustfmt."

    $vsRoots = @('C:/Program Files/Microsoft Visual Studio/2022/Community', 'C:/Program Files/Microsoft Visual Studio/2022/BuildTools', 'C:/Program Files/Microsoft Visual Studio/18/Community')
    $clCandidates = @($vsRoots | ForEach-Object { Get-ChildItem -Path "$_/VC/Tools/MSVC/*/bin/Hostx64/x64/cl.exe" -ErrorAction SilentlyContinue | ForEach-Object FullName })
    $cl = Find-Tool 'cl.exe' $clCandidates
    Record-Check 'MSVC x64 compiler' ([bool]$cl) "$cl; use an x64 developer shell for the build."
    $rcCandidates = @(Get-ChildItem -Path 'C:/Program Files (x86)/Windows Kits/10/bin/*/x64/rc.exe' -ErrorAction SilentlyContinue | ForEach-Object FullName)
    $rc = Find-Tool 'rc.exe' $rcCandidates
    Record-Check 'Windows SDK resource compiler' ([bool]$rc) "$rc"
    $cmakeCandidates = @($vsRoots | ForEach-Object { "$_/Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin/cmake.exe" })
    $cmakeCandidates += 'C:/Program Files/CMake/bin/cmake.exe'
    $cmake = Find-Tool 'cmake.exe' $cmakeCandidates
    Record-Check 'CMake' ([bool]$cmake) "$cmake"
    $clangCandidates = @($vsRoots | ForEach-Object { "$_/VC/Tools/Llvm/x64/bin/libclang.dll" })
    $clangCandidates += 'C:/Program Files/LLVM/bin/libclang.dll'
    if ($env:LIBCLANG_PATH) { $clangCandidates = @((Join-Path $env:LIBCLANG_PATH 'libclang.dll')) + $clangCandidates }
    $libclang = $clangCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    Record-Check 'libclang' ([bool]$libclang) "$libclang"
    $nativeRoot = Join-Path $repo "vcpkg_installed/$triplet"
    $nativeReady = (Test-Path -LiteralPath (Join-Path $nativeRoot 'lib/torrent-rasterbar.lib')) -and (Test-Path -LiteralPath (Join-Path $nativeRoot 'lib/libssl.lib'))
    Record-Check 'Pinned vcpkg libraries' $nativeReady "Install project manifest with triplet $triplet; see security/WINDOWS-BASELINE.md."

    $report = [ordered]@{
        recordedAt = [DateTime]::UtcNow.ToString('o'); head = "$head"; baseline = $baseline
        streamServerRevision = $serverRevision; checks = @($checks.ToArray())
        buildRequested = [bool]$Build; buildStatus = 'NOT_RUN'; runtimeStatus = 'NOT_RUN'
        originalRuntimeInputs = $originalInputs
    }
    $reportFile = Join-Path $outputDir 'result.json'
    $report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $reportFile -Encoding UTF8
    if (@($checks | Where-Object { -not $_.passed }).Count -gt 0) {
        Write-Host "Prerequisites incomplete. Details: $reportFile"
        exit 2
    }
    if ($Build) {
        $env:PATH = (Split-Path -Parent $cargo) + ';' + (Split-Path -Parent $cmake) + ';' + $env:PATH
        $env:LIBCLANG_PATH = Split-Path -Parent $libclang
        # Keep downloaded dependencies and Git caches in the writable checkout.
        $env:CARGO_HOME = Join-Path $repo 'artifacts/cargo-home'
        # Do not override TLS, Defender, compiler flags or dependency/backend selection.
        $ErrorActionPreference = 'Continue'
        & $cargo "+$toolchain" build --locked --release --package stremio-native 2>&1 | Tee-Object -FilePath (Join-Path $outputDir 'build.log')
        $buildExit = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        $report.buildStatus = if ($buildExit -eq 0) { 'PASS' } else { 'FAIL' }
        $report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $reportFile -Encoding UTF8
        if ($buildExit -ne 0) { exit $buildExit }
        Write-Host 'Build passed. Application startup, playback and security remain unverified until tested separately.'
    }
} finally {
    $env:PATH = $savedPath
    $env:LIBCLANG_PATH = $savedLibclang
    $env:CARGO_HOME = $savedCargoHome
    Pop-Location
}
