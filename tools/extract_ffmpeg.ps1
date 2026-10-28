$ErrorActionPreference = 'Stop'
$z = Join-Path $env:TEMP 'ffmpeg.zip'
if (-not (Test-Path $z)) {
    Invoke-WebRequest -Uri 'https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip' -OutFile $z
}
$d = Join-Path $env:TEMP 'ffmpeg_extract'
if (Test-Path $d) { Remove-Item -Recurse -Force $d }
Expand-Archive -Path $z -DestinationPath $d -Force
$exe = Get-ChildItem -Path $d -Recurse -Filter ffmpeg.exe | Select-Object -First 1
New-Item -ItemType Directory -Force -Path 'tools' | Out-Null
Copy-Item $exe.FullName -Destination 'tools\ffmpeg.exe' -Force
Write-Output ((Get-Item 'tools\ffmpeg.exe').Length)
