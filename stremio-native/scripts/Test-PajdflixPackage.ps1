[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$BuildRoot,
    [Parameter(Mandatory)][string]$ExpectedVersion
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# SKIP_BUILD must never turn an older binary into a differently labelled release.
$executable = Join-Path $BuildRoot 'stremio-native.exe'
$metadata = [Diagnostics.FileVersionInfo]::GetVersionInfo($executable)
if ($metadata.ProductName -ne 'Pajdflix' -or $metadata.FileDescription -ne 'Pajdflix') {
    throw 'Pakowanie przerwane: EXE nie ma nazwy produktu Pajdflix.'
}
$expected = [version]$ExpectedVersion
$actual = [version]$metadata.ProductVersion
if ($actual.Major -ne $expected.Major -or $actual.Minor -ne $expected.Minor -or
    $actual.Build -ne $expected.Build -or ($actual.Revision -gt 0 -and $actual.Revision -ne $expected.Revision)) {
    throw "Pakowanie przerwane: EXE ma wersje $actual zamiast $expected. Zbuduj aktualne zrodla."
}
Write-Host "Metadane pakietu: Pajdflix $ExpectedVersion - OK"
