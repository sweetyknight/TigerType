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
; TSF DLL 驻留在用过输入法的进程里，协议严格相等，不重开就只能出英文：准备页由重启管理器列出
; 仍载着现版或历次改名的 .old 旧版的程序，经用户同意关闭并在安装后重启支持重启的程序；
; 用户不关或关不掉的，由 [Files] 的 BeforeInstall 改名后照常安装。
CloseApplications=yes
RestartApplications=yes
UninstallDisplayName=TigerType 虎码输入法
SetupIconFile={#Icon}
UninstallDisplayIcon={app}\tigertype.ico

[Languages]
Name: "chs"; MessagesFile: "{#Root}\installer\ChineseSimplified.isl"

[Messages]
ApplicationsFound2=以下程序仍在使用旧版 TigerType，不重新打开就只能输入英文。建议让安装程序关闭它们；支持的程序会在安装完成后自动重新打开，其余请手动重开。
FinishedLabel=TigerType 已安装，输入后台已启动并会随登录自动启动。%n%n请在“设置 → 时间和语言 → 语言和区域 → 中文(简体) → 语言选项 → 键盘”中添加“TigerType 虎码”。查字与设置可从开始菜单打开。%n%n升级时未关闭的程序须重新打开才会用上新版本。

[Files]
; 程序与配置图标在前：注册 DLL 时要求同目录已有后台程序与 tigertype.ico（任务栏据此显示虎码图标）。
; 改名在关闭程序之后（BeforeInstall），改早了重启管理器就找不到占用者。
Source: "{#Root}\target\release\tigertype.exe"; DestDir: "{app}"; Flags: ignoreversion uninsrestartdelete; BeforeInstall: Retire('tigertype.exe')
Source: "{#Icon}"; DestDir: "{app}"; DestName: "tigertype.ico"; Flags: ignoreversion
Source: "{#Root}\target\release\tigertype_tsf.dll"; DestDir: "{app}"; Flags: ignoreversion regserver uninsrestartdelete 64bit; BeforeInstall: Retire('tigertype_tsf.dll')
Source: "{#Root}\target\i686-pc-windows-msvc\release\tigertype_tsf.dll"; DestDir: "{app}"; DestName: "tigertype_tsf_x86.dll"; Flags: ignoreversion regserver uninsrestartdelete 32bit; BeforeInstall: Retire('tigertype_tsf_x86.dll')
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
; 历次改名的 *.old 不能列在这里：它们都已登记重启后删除，Inno 会据此拒绝安装、要求先重启；改由 [Code] 登记与清理。

[UninstallDelete]
; 卸载时同样删掉已无人使用的旧版文件，仍被占用的已登记重启后删除。
Type: files; Name: "{app}\*.old"

[Registry]
; 每个用户登录时以其普通权限启动自己的后台（数据在各自 %APPDATA%\TigerType）。
Root: HKLM; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "TigerType"; ValueData: """{app}\tigertype.exe"" serve"; Flags: uninsdeletevalue

[Icons]
Name: "{autoprograms}\TigerType 查字与设置"; Filename: "{app}\tigertype.exe"; Parameters: "lookup-window"; IconFilename: "{app}\tigertype.ico"; Comment: "查字、设置；后台未运行时一并启动"

[Run]
; 后台必须以非提升权限运行（提升后管道所有者为 Administrators，宿主会拒绝连接）。
Filename: "{app}\tigertype.exe"; Parameters: "serve"; Flags: runasoriginaluser nowait; StatusMsg: "正在启动输入后台……"

[UninstallRun]
; 卸载程序为提升权限，停止请求实测可被普通权限的后台接受；失败时后台运行到注销，程序在重启后删除。
Filename: "{app}\tigertype.exe"; Parameters: "stop"; Flags: runhidden; RunOnceId: "StopBackend"

[Code]
// 仍被未关闭程序占用的程序与 DLL 不能覆盖但可以改名：改名后新进程即用新版本，旧文件登记为重启后删除。
// 改名失败时不处理，由 Inno 照常报告文件占用并让用户重试或取消。
procedure Retire(const Name: String);
var
  Path, Old: String;
begin
  Path := ExpandConstant('{app}\') + Name;
  if (not FileExists(Path)) or DeleteFile(Path) then
    Exit;
  Old := Path + '.' + GetDateTimeString('yyyymmddhhnnss', #0, #0) + '.old';
  if RenameFile(Path, Old) then
    RestartReplace(Old, '');
end;

// 历次改名的旧版程序与 DLL 仍驻留在当时已打开的程序里，交给重启管理器一并检测；现名文件由 [Files] 自动登记。
procedure RegisterExtraCloseApplicationsResources;
var
  Find: TFindRec;
begin
  if FindFirst(ExpandConstant('{app}\*.old'), Find) then
    try
      repeat
        RegisterExtraCloseApplicationsResource(False, ExpandConstant('{app}\') + Find.Name);
      until not FindNext(Find);
    finally
      FindClose(Find);
    end;
end;

// 关闭程序后已无人使用的旧版文件立即删除；仍被占用的删除失败，留待重启时删除（改名时已登记）。
procedure CurStepChanged(CurStep: TSetupStep);
var
  Find: TFindRec;
begin
  if (CurStep = ssPostInstall) and FindFirst(ExpandConstant('{app}\*.old'), Find) then
    try
      repeat
        DeleteFile(ExpandConstant('{app}\') + Find.Name);
      until not FindNext(Find);
    finally
      FindClose(Find);
    end;
end;

// 升级前先以原用户身份停止后台，stop 等到旧后台进程退出才返回，重启管理器随后检测不到它；
// 后台未运行时 stop 失败，忽略即可。
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  if FileExists(ExpandConstant('{app}\tigertype.exe')) then
    ExecAsOriginalUser(ExpandConstant('{app}\tigertype.exe'), 'stop', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := '';
end;
