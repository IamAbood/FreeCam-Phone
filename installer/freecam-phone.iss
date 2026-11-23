; FreeCam Phone Windows installer (Inno Setup 6).
; Build: iscc /DAppVersion=0.1.0 /DBinDir=..\target\release installer\freecam-phone.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef BinDir
  #define BinDir "..\target\release"
#endif

[Setup]
AppId={{9C3A2E61-4F0B-4C55-9B7E-6F2D3E1A0C11}
AppName=FreeCam Phone
AppVersion={#AppVersion}
AppPublisher=FreeCam Phone contributors
DefaultDirName={autopf}\FreeCam Phone
DefaultGroupName=FreeCam Phone
; Registering the camera DLLs writes HKLM, so the installer needs admin.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=FreeCamPhone-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
LicenseFile=..\LICENSE
UninstallDisplayIcon={app}\freecam-phone.exe
CloseApplications=yes

[Files]
Source: "{#BinDir}\freecam-phone.exe"; DestDir: "{app}"; Flags: ignoreversion
; regserver = regsvr32 on install, unregister on uninstall.
Source: "{#BinDir}\freecam_vcam_mf.dll"; DestDir: "{app}"; Flags: ignoreversion regserver 64bit
Source: "{#BinDir}\freecam_dshow.dll"; DestDir: "{app}"; Flags: ignoreversion regserver 64bit
Source: "..\LICENSE"; DestDir: "{app}"
Source: "..\crates\vcam-dshow\softcam\LICENSE"; DestDir: "{app}"; DestName: "softcam-LICENSE.txt"
Source: "..\README.md"; DestDir: "{app}"; Flags: isreadme

[Icons]
Name: "{group}\FreeCam Phone"; Filename: "{app}\freecam-phone.exe"
Name: "{group}\Uninstall FreeCam Phone"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\freecam-phone.exe"; Description: "Launch FreeCam Phone"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM freecam-phone.exe /F"; Flags: runhidden; RunOnceId: "KillApp"

[UninstallDelete]
Type: filesandordirs; Name: "{userappdata}\FreeCamPhone"
