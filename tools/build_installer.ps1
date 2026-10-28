# Genera dist\Nullframe-Setup-<versión>.exe: compila la app y el instalador en
# release y anexa nullframe.exe + ffmpeg.exe comprimidos al instalador.
#
#   powershell -ExecutionPolicy Bypass -File tools\build_installer.ps1

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# Toolchain MinGW (w64devkit) usado en este equipo.
$mingw = "D:\mingw\w64devkit\bin"
if (Test-Path $mingw) { $env:PATH = "$env:PATH;$mingw" }

$ffmpeg = Join-Path $root "tools\ffmpeg.exe"
if (-not (Test-Path $ffmpeg)) { throw "Falta tools\ffmpeg.exe" }

$version = (Select-String -Path "Cargo.toml" -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value

Write-Host "compilando nullframe $version (release)..."
cargo +stable-x86_64-pc-windows-gnu build --release --workspace
if ($LASTEXITCODE -ne 0) { throw "cargo build falló" }

$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force $dist | Out-Null
$output = Join-Path $dist "Nullframe-Setup-$version.exe"

Write-Host "empaquetando (comprime FFmpeg, tarda un poco)..."
$pack = Start-Process -FilePath "target\release\nullframe-setup.exe" -Wait -PassThru -NoNewWindow -ArgumentList @(
    "--pack", "`"$output`"", "`"$root\target\release\nullframe.exe`"", "`"$ffmpeg`""
)
if ($pack.ExitCode -ne 0) { throw "El empaquetado falló (código $($pack.ExitCode))" }

$size = (Get-Item $output).Length / 1MB
Write-Host ("listo: {0} ({1:N1} MB)" -f $output, $size)
