#ifndef Version
  #error Version must be supplied by Publish.ps1
#endif
#ifndef PublishDir
  #error PublishDir must be supplied by Publish.ps1
#endif
#ifndef OutputDir
  #error OutputDir must be supplied by Publish.ps1
#endif

#define ProductName "Moa Launcher"
#define ProductPublisher "Moa"
#define ProductExe "MoaLauncher.exe"

[Setup]
AppId={{A2C320F8-70C5-4D90-AE3C-0EC94C579A63}
AppName={#ProductName}
AppVersion={#Version}
AppPublisher={#ProductPublisher}
AppPublisherURL=https://github.com/hebory/moalauncher
VersionInfoVersion={#Version}
SetupIconFile=..\src\MoaLauncher.App\Assets\moa.ico
DefaultDirName={localappdata}\Programs\MoaLauncher
DefaultGroupName={#ProductName}
PrivilegesRequired=lowest
ArchitecturesInstallIn64BitMode=x64
ArchitecturesAllowed=x64
OutputDir={#OutputDir}
OutputBaseFilename=MoaLauncher-{#Version}-Setup-x64
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#ProductExe}
CloseApplications=no
RestartApplications=no
SetupLogging=yes
WizardStyle=modern
Compression=lzma2
SolidCompression=yes

[Languages]
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"

[Tasks]
Name: "desktopicon"; Description: "바탕 화면에 바로 가기 만들기"; GroupDescription: "추가 작업:"; Flags: unchecked

[Files]
Source: "{#PublishDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#ProductName}"; Filename: "{app}\{#ProductExe}"
Name: "{autodesktop}\{#ProductName}"; Filename: "{app}\{#ProductExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#ProductExe}"; Description: "{#ProductName} 실행"; Flags: postinstall nowait skipifsilent
