[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$ImagePath)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
$destination = Join-Path $env:LOCALAPPDATA 'Pajdflix\Launcher'
New-Item -ItemType Directory -Force -Path $destination | Out-Null
Copy-Item -LiteralPath $ImagePath -Destination (Join-Path $destination 'Pajdflix.png') -Force

# Format conversion only: keep the supplied artwork and its black background.
Add-Type -AssemblyName System.Drawing
$source = [Drawing.Image]::FromFile((Join-Path $destination 'Pajdflix.png'))
$images = New-Object 'System.Collections.Generic.List[byte[]]'
$sizes = @(256,128,64,48,32,16)
try {
    foreach ($size in $sizes) {
        $bitmap = New-Object Drawing.Bitmap($size,$size)
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        $stream = New-Object IO.MemoryStream
        try {
            $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $graphics.DrawImage($source,0,0,$size,$size)
            $bitmap.Save($stream,[Drawing.Imaging.ImageFormat]::Png)
            $images.Add($stream.ToArray())
        } finally { $graphics.Dispose(); $bitmap.Dispose(); $stream.Dispose() }
    }
} finally { $source.Dispose() }
$iconPath = Join-Path $destination 'Pajdflix-red-P.ico'
$output = [IO.File]::Create($iconPath)
$writer = New-Object IO.BinaryWriter($output)
try {
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$sizes.Count)
    $offset = 6 + 16*$sizes.Count
    for ($index=0; $index -lt $sizes.Count; $index++) {
        $dimension = if ($sizes[$index] -eq 256) { 0 } else { $sizes[$index] }
        $writer.Write([byte]$dimension); $writer.Write([byte]$dimension)
        $writer.Write([byte]0); $writer.Write([byte]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32)
        $writer.Write([uint32]$images[$index].Length); $writer.Write([uint32]$offset)
        $offset += $images[$index].Length
    }
    foreach ($bytes in $images) { $writer.Write([byte[]]$bytes) }
} finally { $writer.Dispose(); $output.Dispose() }

[IO.File]::WriteAllText((Join-Path $destination 'repository.txt'),$repository,[Text.UTF8Encoding]::new($false))
$launcher = Join-Path $destination 'Pajdflix.exe'
$compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
& $compiler /nologo /codepage:65001 /target:winexe /optimize+ /reference:System.Windows.Forms.dll "/win32icon:$iconPath" "/out:$launcher" (Join-Path $PSScriptRoot 'launcher\PajdflixLauncher.cs')
if ($LASTEXITCODE -ne 0) { throw 'Could not build the Pajdflix launcher.' }

$pinned = Join-Path $env:APPDATA 'Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\Pajdflix.lnk'
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Pajdflix.lnk'
$shell = New-Object -ComObject WScript.Shell
if (Test-Path -LiteralPath $pinned) {
    $backup = Join-Path $destination ('Pajdflix-pinned-backup-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.lnk')
    Copy-Item -LiteralPath $pinned -Destination $backup
    # Updating the existing link preserves its pin and explicit AppUserModelID.
    $link = $shell.CreateShortcut($pinned)
    $link.TargetPath = $launcher
    $link.Arguments = ''
    $link.WorkingDirectory = $destination
    $link.IconLocation = "$iconPath,0"
    $link.Description = 'Pajdflix - najnowsza gotowa wersja z projektu'
    $link.Save()
    Copy-Item -LiteralPath $pinned -Destination $startMenu -Force
} else {
    $link = $shell.CreateShortcut($startMenu)
    $link.TargetPath = $launcher; $link.WorkingDirectory = $destination
    $link.IconLocation = "$iconPath,0"
    $link.Description = 'Pajdflix - najnowsza gotowa wersja z projektu'
    $link.Save()
}

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class PajdflixShellNotify {
    [DllImport("shell32.dll", CharSet=CharSet.Unicode)]
    public static extern void SHChangeNotify(uint change, uint flags, string first, IntPtr second);
}
'@
if (Test-Path -LiteralPath $pinned) { [PajdflixShellNotify]::SHChangeNotify(0x2000,0x1005,$pinned,[IntPtr]::Zero) }
[PajdflixShellNotify]::SHChangeNotify(0x08000000,0,[string]$null,[IntPtr]::Zero)
Write-Output "Launcher: $launcher"
Write-Output "Icon: $iconPath"
Write-Output "Start menu: $startMenu"
Write-Output ('Existing taskbar pin updated: ' + (Test-Path -LiteralPath $pinned))
