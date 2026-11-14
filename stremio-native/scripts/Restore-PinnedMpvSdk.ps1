[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$lock = Get-Content -LiteralPath (Join-Path $repo 'mpv.lock.json') -Raw | ConvertFrom-Json
$artifact = $lock.artifacts.'windows-x86_64-v3-dynamic'
$work = Join-Path $repo 'artifacts/mpv-sdk-recovery'
$cache = Join-Path $repo ('target/.mpv-cache/' + $lock.distribution.sha256.Substring(0,16))
New-Item -ItemType Directory -Force -Path $work,$cache | Out-Null

function Assert-Hash([string]$Path, [string]$Expected) {
    if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -ne $Expected) { throw "SHA-256 mismatch: $Path" }
}
function Get-VerifiedFile([string]$Url, [string]$Path, [string]$Hash) {
    if (Test-Path -LiteralPath $Path) { Assert-Hash $Path $Hash; return }
    $partial = $Path + '.part'
    & curl.exe --fail --location --silent --show-error --retry 2 --output $partial $Url
    if ($LASTEXITCODE -ne 0) { throw "Download failed: $Url" }
    Assert-Hash $partial $Hash
    Move-Item -LiteralPath $partial -Destination $Path
}

$dlltool = Join-Path $env:ProgramFiles 'LLVM/bin/llvm-dlltool.exe'
if (-not (Test-Path -LiteralPath $dlltool)) { throw 'llvm-dlltool.exe is required; the recorded recovery used LLVM 23.1.5.' }
$release = Join-Path $work 'stremio-v1.0.6.zip'
Get-VerifiedFile 'https://github.com/stremio-native/stremio-native/releases/download/v1.0.6/stremio-native-v1.0.6-x86_64-pc-windows-msvc.zip' $release 'fe63a114413a8600935b89fd449b2966a1ff3a1ff70d510c9310f2e21624f70f'
$unpacker = Join-Path $work 'innounp-2.zip'
Get-VerifiedFile 'https://github.com/jrathlev/InnoUnpacker-Windows-GUI/releases/download/ui_2_2_13/innounp-2.zip' $unpacker 'f8e45c9676bebc7d7570108ab572673b541fcf7a7d3d24a04319ac6b5315b4e8'
$releaseDir = Join-Path $work 'release'
$toolDir = Join-Path $work 'unpacker'
Expand-Archive -LiteralPath $release -DestinationPath $releaseDir -Force
Expand-Archive -LiteralPath $unpacker -DestinationPath $toolDir -Force
$extracted = Join-Path $work 'extracted'
New-Item -ItemType Directory -Force -Path $extracted | Out-Null
& (Join-Path $toolDir 'innounp.exe') -x -b "-d$extracted" (Join-Path $releaseDir 'stremio-installer.exe') *> (Join-Path $work 'extract.log')
if ($LASTEXITCODE -ne 0) { throw 'Cannot extract the verified original Stremio release.' }
$runtime = Join-Path $extracted '{app}/libmpv-2.dll'
Assert-Hash $runtime $artifact.runtimeLibrary.sha256
$libDir = Join-Path $cache 'lib'
$binDir = Join-Path $cache 'bin'
New-Item -ItemType Directory -Force -Path $libDir,$binDir | Out-Null
$importLibrary = Join-Path $libDir 'mpv.lib'
& $dlltool -m 'i386:x86-64' -D 'libmpv-2.dll' -d (Join-Path $repo 'playback-mpv/mpv.def') -l $importLibrary
if ($LASTEXITCODE -ne 0) { throw 'Import library generation failed.' }
Assert-Hash $importLibrary $artifact.recoveredImportLibrarySha256
Copy-Item -LiteralPath $runtime -Destination (Join-Path $binDir 'libmpv-2.dll') -Force
Write-Host 'Recovered SDK: original pinned mpv DLL and separately pinned generated import library.'
