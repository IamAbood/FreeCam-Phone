# Registers the locally built virtual camera DLLs (run from an elevated PowerShell).
#   .\scripts\register-dev.ps1            # register target\release DLLs
#   .\scripts\register-dev.ps1 -Unregister
# The MF DLL must live somewhere the LOCAL SERVICE account can read (the Frame
# Server loads it), so it is copied to %ProgramData%\FreeCamPhone first.
param([switch]$Unregister, [string]$Profile = "release")
$ErrorActionPreference = "Stop"
$src = Join-Path $PSScriptRoot "..\target\$Profile"
$dst = Join-Path $env:ProgramData "FreeCamPhone"
$dlls = "freecam_vcam_mf.dll", "freecam_dshow.dll"
if ($Unregister) {
    foreach ($d in $dlls) { if (Test-Path "$dst\$d") { regsvr32 /s /u "$dst\$d" } }
    Write-Host "Unregistered."
    return
}
New-Item -ItemType Directory -Force $dst | Out-Null
foreach ($d in $dlls) {
    Copy-Item (Join-Path $src $d) $dst -Force
    $p = Start-Process regsvr32 -ArgumentList "/s", "`"$dst\$d`"" -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "regsvr32 failed for $d ($($p.ExitCode))" }
}
Write-Host "Registered FreeCam Phone Camera (MF + DirectShow) from $dst"
