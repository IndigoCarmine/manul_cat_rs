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

; "x64compatible" (native x64 + ARM64 x64-emulation) is preferred but only exists
; on Inno Setup 6.3+; older compilers error on it, so fall back to "x64" there.
#if Ver >= EncodeVer(6,3,0)
  #define ArchId "x64compatible"
#else
  #define ArchId "x64"
#endif

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
ArchitecturesAllowed={#ArchId}
ArchitecturesInstallIn64BitMode={#ArchId}
; The [Registry] section below (opt-in "associate" task) changes file associations.
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[CustomMessages]
english.AssociateFiles=Associate GROMACS / molecular files (.gro, .pdb, .mol2, .top, .itp, .ndx, .xtc) with Manul
japanese.AssociateFiles=GROMACS・分子ファイル (.gro, .pdb, .mol2, .top, .itp, .ndx, .xtc) を Manul に関連付ける
english.AssociateGroup=File associations:
japanese.AssociateGroup=ファイルの関連付け:

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
; Off by default: .pdb/.mol2/.itp may already be handled by other tools (PyMOL, VMD, ...).
Name: "associate"; Description: "{cm:AssociateFiles}"; GroupDescription: "{cm:AssociateGroup}"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\resources\Manuru.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README_ja.md"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; A single ProgID for every molecular file type Manul opens. `%1` passes the
; double-clicked file to the exe, which main.rs loads via KuromameApp::load_paths.
Root: HKA; Subkey: "Software\Classes\Manul.MolecularFile"; ValueType: string; ValueName: ""; ValueData: "Molecular structure file"; Flags: uninsdeletekey; Tasks: associate
Root: HKA; Subkey: "Software\Classes\Manul.MolecularFile\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#MyAppExeName},0"; Tasks: associate
Root: HKA; Subkey: "Software\Classes\Manul.MolecularFile\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""; Tasks: associate
; Point each extension's default handler at the ProgID and register it in the
; "Open with" list. uninsdeletevalue removes only the values we wrote on uninstall.
Root: HKA; Subkey: "Software\Classes\.gro"; ValueType: string; ValueName: ""; ValueData: "Manul.MolecularFile"; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.gro\OpenWithProgids"; ValueType: string; ValueName: "Manul.MolecularFile"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.pdb"; ValueType: string; ValueName: ""; ValueData: "Manul.MolecularFile"; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.pdb\OpenWithProgids"; ValueType: string; ValueName: "Manul.MolecularFile"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.mol2"; ValueType: string; ValueName: ""; ValueData: "Manul.MolecularFile"; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.mol2\OpenWithProgids"; ValueType: string; ValueName: "Manul.MolecularFile"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.top"; ValueType: string; ValueName: ""; ValueData: "Manul.MolecularFile"; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.top\OpenWithProgids"; ValueType: string; ValueName: "Manul.MolecularFile"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.itp"; ValueType: string; ValueName: ""; ValueData: "Manul.MolecularFile"; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.itp\OpenWithProgids"; ValueType: string; ValueName: "Manul.MolecularFile"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.ndx"; ValueType: string; ValueName: ""; ValueData: "Manul.MolecularFile"; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.ndx\OpenWithProgids"; ValueType: string; ValueName: "Manul.MolecularFile"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.xtc"; ValueType: string; ValueName: ""; ValueData: "Manul.MolecularFile"; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\.xtc\OpenWithProgids"; ValueType: string; ValueName: "Manul.MolecularFile"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associate

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\Manuru.ico"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\Manuru.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent
