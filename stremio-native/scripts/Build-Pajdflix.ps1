<#
.SYNOPSIS
    Builds Pajdflix and leaves it ready to start from the desktop shortcut.

.DESCRIPTION
    Fast: cargo profile `local` (opt-level 2, no LTO, incremental) -> target\local.
          The first run compiles all dependencies; later UI/app changes take minutes.
    Full: cargo profile `release` (thin LTO, opt-level 3) -> target\release.
          Use for the final/everyday-stable build; app changes take ~30 minutes.

    Steps: close the running Pajdflix from this repository (a running EXE cannot be
    overwritten), build, verify the executable, refresh the launcher and the desktop
    shortcut. The launcher always starts the newest finished build of either profile.

.EXAMPLE
    .\scripts\Build-Pajdflix.ps1            # fast
    .\scripts\Build-Pajdflix.ps1 -Mode Full
    .\scripts\Build-Pajdflix.ps1 -Run       # build, then start the app
#>
[CmdletBinding()]
param(
    [ValidateSet('Fast', 'Full')][string]$Mode = 'Fast',
    [switch]$Run,
    [switch]$KeepRunningApp,
    # Useful for portable builds/CI where desktop and launcher paths are read-only.
    [switch]$SkipShortcut
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repository = Split-Path -Parent $PSScriptRoot
$profileName = if ($Mode -eq 'Fast') { 'local' } else { 'release' }
$profileDir = Join-Path $repository "target\$profileName"
$exe = Join-Path $profileDir 'stremio-native.exe'
$launcherDir = Join-Path $env:LOCALAPPDATA 'Pajdflix\Launcher'
$launcher = Join-Path $launcherDir 'Pajdflix.exe'
$launcherSource = Join-Path $PSScriptRoot 'launcher\PajdflixLauncher.cs'

function Write-Step([string]$text) { Write-Host "`n==> $text" -ForegroundColor Cyan }
function Fail([string]$text) { Write-Host "`nBLAD: $text" -ForegroundColor Red; exit 1 }

# 1. A running build locks its EXE; cargo would compile for half an hour and then fail to replace it.
Write-Step 'Sprawdzanie uruchomionej aplikacji'
$targetRoot = (Join-Path $repository 'target') + '\'
$running = @(Get-Process stremio-native -ErrorAction SilentlyContinue | Where-Object {
    try { $_.Path -and $_.Path.StartsWith($targetRoot, [StringComparison]::OrdinalIgnoreCase) } catch { $false }
})
if ($running.Count -gt 0) {
    if ($KeepRunningApp) { Fail 'Pajdflix jest uruchomiony (takze w zasobniku). Zamknij go albo uruchom skrypt bez -KeepRunningApp.' }
    Write-Host "Zamykam dzialajacy Pajdflix ($($running.Count) proc.)"
    $running | ForEach-Object { [void]$_.CloseMainWindow() }
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $deadline -and ($running | Where-Object { -not $_.HasExited })) { Start-Sleep -Milliseconds 300 }
    $running | Where-Object { -not $_.HasExited } | Stop-Process -Force
} else { Write-Host 'Nie dziala - OK' }

# 2. Build.
$cargoBusy = @(Get-Process cargo -ErrorAction SilentlyContinue)
if ($cargoBusy.Count -gt 0) { Fail 'Dziala juz inny proces cargo. Poczekaj na jego zakonczenie przed kolejna kompilacja.' }

Write-Step "Kompilacja: $Mode (profil $profileName)"
if ($Mode -eq 'Fast' -and -not (Test-Path (Join-Path $profileDir 'stremio-native.exe'))) {
    Write-Host 'Pierwsza szybka kompilacja buduje wszystkie zaleznosci - kolejne beda o wiele krotsze.' -ForegroundColor Yellow
}
$started = Get-Date
Push-Location $repository
try {
    & cargo build --profile $profileName --package stremio-native
    $cargoExit = $LASTEXITCODE
} finally { Pop-Location }
$elapsed = (Get-Date) - $started
if ($cargoExit -ne 0) { Fail "cargo zakonczyl sie kodem $cargoExit (czas: $($elapsed.ToString('hh\:mm\:ss')))." }

# 3. Verify the result.
Write-Step 'Weryfikacja'
if (-not (Test-Path $exe)) { Fail "Brak $exe" }
$bytes = New-Object byte[] 2
$stream = [IO.File]::OpenRead($exe)
try { [void]$stream.Read($bytes, 0, 2) } finally { $stream.Dispose() }
if ($bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) { Fail "$exe nie jest poprawnym plikiem EXE." }
if (-not (Test-Path (Join-Path $profileDir 'libmpv-2.dll'))) { Fail "Brak libmpv-2.dll obok $exe" }
$exeTime = (Get-Item $exe).LastWriteTime
Write-Host "EXE: $exe ($([math]::Round((Get-Item $exe).Length / 1MB)) MB, $exeTime)"
if ($exeTime -lt $started) { Write-Host 'Bez zmian w kodzie - EXE byl juz aktualny.' }

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (Test-Path $vswhere) {
    $vs = & $vswhere -latest -products * -property installationPath
    $dumpbin = if ($vs) { Get-ChildItem (Join-Path $vs 'VC\Tools\MSVC') -Recurse -Filter dumpbin.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like '*Hostx64\x64*' } | Select-Object -First 1 }
    if ($dumpbin) {
        # Skia's static ICU must win over the system icuuc.dll (see .cargo/config.toml).
        $imports = & $dumpbin.FullName /nologo /dependents $exe
        if ($imports -match 'icuuc\.dll|icu\.dll') { Fail 'EXE importuje systemowe ICU - Skia uzywa zlego ICU (patrz .cargo/config.toml).' }
        Write-Host 'Importy DLL: OK (bez systemowego ICU)'
    }
}

# 4. Launcher + desktop shortcut.
Write-Step 'Skrot na pulpicie'
if ($SkipShortcut) {
    Write-Host "Pominieto zapis skrotu. Uruchom aplikacje bezposrednio: $exe"
} else {
if (-not (Test-Path (Join-Path $launcherDir 'Pajdflix-red-P.ico'))) {
    Write-Host 'Launcher nie jest zainstalowany - uruchom raz scripts\Install-PajdflixShortcut.ps1 -ImagePath <ikona.png>.' -ForegroundColor Yellow
} else {
    if (-not (Test-Path $launcher) -or (Get-Item $launcherSource).LastWriteTime -gt (Get-Item $launcher).LastWriteTime) {
        $compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
        & $compiler /nologo /codepage:65001 /target:winexe /optimize+ /reference:System.Windows.Forms.dll `
            "/win32icon:$(Join-Path $launcherDir 'Pajdflix-red-P.ico')" "/out:$launcher" $launcherSource
        if ($LASTEXITCODE -ne 0) { Fail 'Nie udalo sie zbudowac launchera.' }
        [IO.File]::WriteAllText((Join-Path $launcherDir 'repository.txt'), $repository, (New-Object Text.UTF8Encoding($false)))
        Write-Host 'Launcher zaktualizowany'
    }
    $shortcut = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Pajdflix.lnk'
    $link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
    $link.TargetPath = $launcher
    $link.WorkingDirectory = $launcherDir
    $link.IconLocation = (Join-Path $launcherDir 'Pajdflix-red-P.ico') + ',0'
    $link.Description = 'Pajdflix - najnowsza gotowa wersja z projektu'
    $link.Save()
    $resolver = Start-Process -FilePath $launcher -ArgumentList '--resolve-only' -Wait -PassThru
    $selected = (Get-Content (Join-Path $launcherDir 'last-selection.txt') | Select-Object -Last 1)
    Write-Host "Skrot: $shortcut"
    Write-Host "Skrot uruchomi: $selected"
    if ($resolver.ExitCode -ne 0 -or -not [string]::Equals($selected, $exe, [StringComparison]::OrdinalIgnoreCase)) {
        Write-Host 'Uwaga: skrot wybiera inny build niz wlasnie zbudowany (inny profil ma nowszy UI).' -ForegroundColor Yellow
    }
}
}

Write-Host "`nGOTOWE - $Mode w $($elapsed.ToString('hh\:mm\:ss')). EXE: $exe" -ForegroundColor Green
if ($Run) { Start-Process -FilePath $exe -WorkingDirectory $profileDir }
