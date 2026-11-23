# Builds the release APK and the Windows installer into .\dist
#   .\scripts\build-release.ps1 [-Version 0.1.0]
# Needs: Rust (stable), cargo-ndk, Android SDK/NDK + JDK 17+, Inno Setup 6 (iscc on PATH or in ISCC env var).
param([string]$Version = "0.1.0")
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root
git submodule update --init --recursive
New-Item -ItemType Directory -Force dist | Out-Null

cargo build --release -p receiver -p vcam-mf -p vcam-dshow
if ($LASTEXITCODE) { throw "cargo build failed" }

Push-Location android
$parts = $Version.Split(".")
$code = [int]$parts[0] * 10000 + [int]$parts[1] * 100 + [int]$parts[2]
& .\gradlew.bat assembleRelease "-PversionName=$Version" "-PversionCode=$code"
if ($LASTEXITCODE) { throw "gradle build failed" }
Pop-Location
Copy-Item android\app\build\outputs\apk\release\app-release.apk "dist\FreeCamPhone-$Version.apk" -Force

$iscc = if ($env:ISCC) { $env:ISCC } else { "iscc" }
& $iscc "/DAppVersion=$Version" "/DBinDir=$root\target\release" installer\freecam-phone.iss
if ($LASTEXITCODE) { throw "iscc failed" }
Get-ChildItem dist
