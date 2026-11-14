[CmdletBinding()]
param([string]$Encoder, [string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Encoder) { $Encoder = Join-Path $repo 'artifacts/ffmpeg-reviewed/sdk/ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe' }
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repo 'artifacts/legal-media-fixture' }
$expected = '3256173f3f8bffd7df12227c68adf68025edb1832273a9530688a7bb1ed8edec'
if ((Get-FileHash -LiteralPath $Encoder -Algorithm SHA256).Hash -ne $expected) { throw 'Encoder is not the reviewed FFmpeg 9.0.2 binary.' }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$subtitle = "1`n00:00:00,000 --> 00:00:08,000`nLegalny test napisów: ąćęłńóśźż.`n`n2`n00:00:14,000 --> 00:00:23,000`nDruga plansza po przewinięciu.`n"
$subtitlePath = Join-Path $OutputDirectory 'fixture.srt'
$videoPath = Join-Path $OutputDirectory 'fixture.mp4'
[IO.File]::WriteAllText($subtitlePath, $subtitle, [Text.UTF8Encoding]::new($false))
# Entirely generated test pattern and silent audio; no third-party media.
& $Encoder -hide_banner -loglevel error -f lavfi -i 'testsrc2=size=320x180:rate=24' -f lavfi -i 'anullsrc=r=48000:cl=stereo' -i $subtitlePath -t 24 -map '0:v' -map '1:a' -map '2:s' -c:v libx264 -preset ultrafast -crf 20 -pix_fmt yuv420p -c:a aac -c:s mov_text -movflags +faststart -y $videoPath
if ($LASTEXITCODE -ne 0) { throw 'Could not generate the legal fixture.' }
[ordered]@{
    source = 'Generated testsrc2 + silent anullsrc; original Polish test captions'
    encoderVersion = '9.0.2-essentials_build-www.gyan.dev'
    encoderSha256 = $expected
    archiveUrl = 'https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip'
    archiveSha256 = '60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba'
    videoSha256 = (Get-FileHash -LiteralPath $videoPath -Algorithm SHA256).Hash.ToLowerInvariant()
    subtitleSha256 = (Get-FileHash -LiteralPath $subtitlePath -Algorithm SHA256).Hash.ToLowerInvariant()
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $OutputDirectory 'provenance.json') -Encoding UTF8
Write-Host "Legal test media generated: $videoPath"
