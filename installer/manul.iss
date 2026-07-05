; Inno Setup script for Manul - A Molecular Viewer for GROMACS
;
; Build locally with:
;   iscc installer\manul.iss
; Override the version at build time (used by CI):
;   iscc /DMyAppVersion=0.7.0 installer\manul.iss
;
; Expects the release binary at target\release\manul_cat_rs.exe
; (run `cargo build --release` first).

#ifndef MyAppVersion
  #define MyAppVersion "0.7.0"
#endif

#define MyAppName "Manul"
#define MyAppFullName "Manul - A Molecular Viewer for GROMACS"
#define MyAppPublisher "Yuhei Yamada (Indigo Carmine)"
#define MyAppURL "https://github.com/IndigoCarmine/manul_cat_rs"
#define MyAppExeName "manul_cat_rs.exe"

[Setup]
; A unique AppId keeps upgrades/uninstall consistent across versions. Do not change it.
AppId={{9F3C1A2B-7D64-4E58-9C21-4A0E2F7B8D33}
AppName={#MyAppFullName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppFullName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}/releases
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
; Per-user install by default when run without admin; falls back cleanly otherwise.
PrivilegesRequiredOverridesAllowed=dialog commandline
OutputDir=..\dist
OutputBaseFilename=manul-setup-{#MyAppVersion}-windows-x64
SetupIconFile=..\resources\Manuru.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppFullName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\resources\Manuru.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README_ja.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\Manuru.ico"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\Manuru.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent
