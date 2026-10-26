; TigerType 安装包定义（Inno Setup 6.7）。由打包驱动编译：
;   cargo run --locked --offline -p tigertype-app --example package
; 驱动先构建 release 程序、校验锁定码表、字体与语言文件并现画虎码图标，再以 /DVersion /DIcon 调用 ISCC。
; 固定装到 Program Files：自带 ALL APPLICATION PACKAGES 读/执行（UWP 宿主可加载 DLL），非管理员不可写
; （DLL 会进入管理员进程，可写目录即提权途径），因此不提供改目录页。
#ifndef Version
  #error 请用打包驱动编译（缺少 /DVersion）
#endif
#ifndef Icon
  #error 请用打包驱动编译（缺少 /DIcon）
#endif
#define Root AddBackslash(SourcePath) + ".."

[Setup]
AppId={{143E92AE-4E95-45B8-BE28-51BEC78A6475}
AppName=TigerType
AppVersion={#Version}
AppVerName=TigerType {#Version}
VersionInfoVersion={#Version}
DefaultDirName={autopf}\TigerType
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=admin
; 后台只有 x64；x86 DLL 供 64 位系统上的 32 位宿主使用。
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#Root}\target\package
OutputBaseFilename=TigerType-Setup-{#Version}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ShowLanguageDialog=no
; TSF DLL 驻留在所有用过输入法的进程里，重启管理器会要求关掉它们；改由安装前改名旧文件处理。
CloseApplications=no
UninstallDisplayName=TigerType 虎码输入法
SetupIconFile={#Icon}
UninstallDisplayIcon={app}\tigertype.ico

[Languages]
Name: "chs"; MessagesFile: "{#Root}\installer\ChineseSimplified.isl"

[Messages]
FinishedLabel=TigerType 已安装，输入后台已启动并会随登录自动启动。%n%n请在“设置 → 时间和语言 → 语言和区域 → 中文(简体) → 语言选项 → 键盘”中添加“TigerType 虎码”。查字与设置可从开始菜单打开。%n%n升级时，安装前已打开的程序须重新打开才会用上新版本。

[Files]
; 程序与配置图标在前：注册 DLL 时要求同目录已有后台程序与 tigertype.ico（任务栏据此显示虎码图标）。
Source: "{#Root}\target\release\tigertype.exe"; DestDir: "{app}"; Flags: ignoreversion uninsrestartdelete
Source: "{#Icon}"; DestDir: "{app}"; DestName: "tigertype.ico"; Flags: ignoreversion
Source: "{#Root}\target\release\tigertype_tsf.dll"; DestDir: "{app}"; Flags: ignoreversion regserver uninsrestartdelete 64bit
Source: "{#Root}\target\i686-pc-windows-msvc\release\tigertype_tsf.dll"; DestDir: "{app}"; DestName: "tigertype_tsf_x86.dll"; Flags: ignoreversion regserver uninsrestartdelete 32bit
; 锁定码表原文，后台启动时逐个核对大小/SHA256 后解析；清单变动须同步此处（打包驱动会核对）。
Source: "{#Root}\resources\tiger\tiger.dict.yaml"; DestDir: "{app}\tiger"; Flags: ignoreversion
Source: "{#Root}\resources\tiger\tigress_ci.dict.yaml"; DestDir: "{app}\tiger"; Flags: ignoreversion
Source: "{#Root}\resources\tiger\tigress_simp_ci.dict.yaml"; DestDir: "{app}\tiger"; Flags: ignoreversion
Source: "{#Root}\resources\tiger\core2022.dict.yaml"; DestDir: "{app}\tiger"; Flags: ignoreversion
Source: "{#Root}\resources\tiger\PY_c.dict.yaml"; DestDir: "{app}\tiger"; Flags: ignoreversion
Source: "{#Root}\resources\tiger\opencc\hu_cf.txt"; DestDir: "{app}\tiger\opencc"; Flags: ignoreversion
Source: "{#Root}\resources\tiger\opencc\PYCharacters.txt"; DestDir: "{app}\tiger\opencc"; Flags: ignoreversion
Source: "{#Root}\resources\tiger\opencc\STCharacters_Tu.txt"; DestDir: "{app}\tiger\opencc"; Flags: ignoreversion
Source: "{#Root}\resources\tiger\opencc\TSCharacters.txt"; DestDir: "{app}\tiger\opencc"; Flags: ignoreversion
; 字体为全机安装（各用户与 UWP 宿主可见）；已有同名文件（如 Win11 自带 Cascadia Mono）不覆盖，卸载不删除。
Source: "{#Root}\resources\fonts\LXGWWenKaiGBScreen.ttf"; DestDir: "{autofonts}"; FontInstall: "LXGW WenKai GB Screen"; Flags: onlyifdoesntexist uninsneveruninstall
Source: "{#Root}\resources\fonts\CascadiaMono.ttf"; DestDir: "{autofonts}"; FontInstall: "Cascadia Mono"; Flags: onlyifdoesntexist uninsneveruninstall
Source: "{#Root}\resources\fonts\TumanPUA.ttf"; DestDir: "{autofonts}"; FontInstall: "TumanPUA"; Flags: onlyifdoesntexist uninsneveruninstall

[InstallDelete]
; 旧版的 JSON 数据包已由 tiger 码表目录取代。
Type: files; Name: "{app}\tiger-v1.json"

[Registry]
; 每个用户登录时以其普通权限启动自己的后台（数据在各自 %APPDATA%\TigerType）。
Root: HKLM; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "TigerType"; ValueData: """{app}\tigertype.exe"" serve"; Flags: uninsdeletevalue

[Icons]
Name: "{autoprograms}\TigerType 查字与设置"; Filename: "{app}\tigertype.exe"; Parameters: "lookup-window"; IconFilename: "{app}\tigertype.ico"; Comment: "查字、设置；后台未运行时一并启动"

[Run]
; 后台必须以非提升权限运行（提升后管道所有者为 Administrators，宿主会拒绝连接）。
Filename: "{app}\tigertype.exe"; Parameters: "serve"; Flags: runasoriginaluser nowait; StatusMsg: "正在启动输入后台……"

[UninstallRun]
; 卸载程序为提升权限，停止请求能否被普通权限的后台接受尚未实测；失败时后台运行到注销，程序在重启后删除。
Filename: "{app}\tigertype.exe"; Parameters: "stop"; Flags: runhidden; RunOnceId: "StopBackend"

[Code]
// 运行中的程序与已加载的 DLL 不能覆盖但可以改名：改名后新进程即用新版本，旧文件登记为重启后删除。
function Retire(const Name: String): String;
var
  Path, Old: String;
begin
  Result := '';
  Path := ExpandConstant('{app}\') + Name;
  if (not FileExists(Path)) or DeleteFile(Path) then
    Exit;
  Old := Path + '.' + GetDateTimeString('yyyymmddhhnnss', #0, #0) + '.old';
  if RenameFile(Path, Old) then
    RestartReplace(Old, '')
  else
    Result := '无法替换正在使用的 ' + Path;
end;

// 升级前先以原用户身份停止后台；后台未运行时 stop 失败，忽略即可。
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  if FileExists(ExpandConstant('{app}\tigertype.exe')) then
    ExecAsOriginalUser(ExpandConstant('{app}\tigertype.exe'), 'stop', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := Retire('tigertype.exe');
  if Result = '' then
    Result := Retire('tigertype_tsf.dll');
  if Result = '' then
    Result := Retire('tigertype_tsf_x86.dll');
end;
