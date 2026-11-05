//! Linux 交付契约：按构建发行版分包，原始码表与全部字体锁定；安装器不把新系统产物冒充旧系统兼容。
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};
use tigertype_backend::{Result, import, resources, storage};
pub const PREFIX: &str = "/opt/tigertype";
pub const PROGRAM: &str = "/opt/tigertype/bin/tigertype-linux-app";
pub const SETUP: &str = "/opt/tigertype/bin/tigertype-setup";
pub const GNOME_UUID: &str = "tigertype@tigertype.local";
// 适配脚本同程序包发布，原始字体不变；扩展代码升级须提升 metadata 的版本并在下次登录加载。
pub const GNOME_FILES: [(&str, &str); 3] = [
    (
        "usr/share/gnome-shell/extensions/tigertype@tigertype.local/metadata.json",
        include_str!("../../input-method/gnome/metadata.json"),
    ),
    (
        "usr/share/gnome-shell/extensions/tigertype@tigertype.local/extension.js",
        include_str!("../../input-method/gnome/extension.js"),
    ),
    (
        "usr/share/gnome-shell/extensions/tigertype@tigertype.local/presentation.js",
        include_str!("../../input-method/gnome/presentation.js"),
    ),
];
pub const IBUS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<component><name>org.freedesktop.IBus.TigerType</name><description>TigerType</description><exec>/opt/tigertype/bin/tigertype-linux-app ibus</exec><version>0.1.0</version><license>personal-use</license><author>TigerType</author><homepage>https://github.com/</homepage><textdomain>tigertype</textdomain><engines><engine><name>tigertype</name><longname>TigerType 虎码</longname><description>虎码单字与词</description><language>zh</language><license>personal-use</license><author>TigerType</author><layout>us</layout><symbol>虎</symbol><rank>0</rank></engine></engines></component>
"#;
pub const ADDON: &str = "[Addon]\nName=TigerType\nCategory=InputMethod\nVersion=0.1.0\nLibrary=tigertype\nType=SharedLibrary\nOnDemand=True\n[Addon/Dependencies]\n0=core\n";
pub const INPUT_METHOD: &str = "[InputMethod]\nName=TigerType 虎码\nIcon=input-keyboard\nLabel=虎\nLangCode=zh_CN\nAddon=tigertype\nConfigurable=False\n";
// StartupNotify 让 GLib 向 Shell 申请激活令牌，后台才能合法前置已打开的设置窗口；
// 窗口 app_id 取程序名 tigertype（native 设置），与本文件名对应，Shell 才能关联名称与图标。
pub const DESKTOP: &str = "[Desktop Entry]\nType=Application\nName=TigerType 虎码\nExec=/opt/tigertype/bin/tigertype-linux-app settings\nIcon=input-keyboard\nTerminal=false\nStartupNotify=true\nCategories=Settings;\n";
/// 受管自启动文件按首行标记识别（任何版本），内容变化后升级/卸载仍能接管或清理本产品的文件。
pub const SESSION_HEADER: &str = "# TigerType managed session";
pub fn managed_session(bytes: &[u8]) -> bool {
    bytes.starts_with(SESSION_HEADER.as_bytes())
}
/// 只有 Fcitx5 需要登录自启动后台；未登记时先待登记，框架确认收下本输入法后改为只启动后台，
/// 不在以后的登录中重新加入用户已删除的条目。IBus 由 daemon 按需拉起，不写自启动。
pub fn session_entry(registered: bool) -> String {
    let command = if registered { "fcitx" } else { "login fcitx" };
    format!(
        "{SESSION_HEADER} v2\n[Desktop Entry]\nType=Application\nName=TigerType\nExec={PROGRAM} {command}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}
pub fn program_files() -> Vec<&'static str> {
    let mut files = vec![
        "opt/tigertype/bin/tigertype-linux-app",
        "opt/tigertype/bin/tigertype-setup",
        "usr/lib/x86_64-linux-gnu/fcitx5/tigertype.so",
        "usr/share/ibus/component/tigertype.xml",
        "usr/share/fcitx5/addon/tigertype.conf",
        "usr/share/fcitx5/inputmethod/tigertype.conf",
        "usr/share/applications/tigertype.desktop",
    ];
    files.extend(GNOME_FILES.iter().map(|entry| entry.0));
    files
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub version: String,
}
impl Target {
    pub fn parse(text: &str) -> Result<Self> {
        let get = |key: &str| -> Result<String> {
            let fields: Vec<_> = text
                .lines()
                .filter_map(|line| line.split_once('='))
                .filter(|(k, _)| *k == key)
                .collect();
            if fields.len() != 1 {
                return Err("发行版标识缺失或重复".into());
            }
            Ok(fields[0].1.trim_matches('"').into())
        };
        let target = Self {
            id: get("ID")?,
            version: get("VERSION_ID")?,
        };
        if !matches!(
            (target.id.as_str(), target.version.as_str()),
            ("ubuntu", "24.04" | "26.04") | ("debian", "13")
        ) {
            return Err("此发行版/版本不在支持目标内".into());
        }
        Ok(target)
    }
    pub fn sources(&self) -> String {
        // 仅使用发行版官方 HTTPS 仓库和预装官方密钥，不修改用户已有软件源配置。
        match self.id.as_str() {
            "ubuntu" => {
                let suite = if self.version == "24.04" { "noble" } else { "resolute" };
                format!("deb [arch=amd64 signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://archive.ubuntu.com/ubuntu {suite} main universe\ndeb [arch=amd64 signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://archive.ubuntu.com/ubuntu {suite}-updates main universe\ndeb [arch=amd64 signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://security.ubuntu.com/ubuntu {suite}-security main universe\n")
            }
            _ => "deb [arch=amd64 signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://deb.debian.org/debian trixie main\ndeb [arch=amd64 signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://deb.debian.org/debian trixie-updates main\ndeb [arch=amd64 signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://security.debian.org/debian-security trixie-security main\n".into(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: u32,
    pub target: Target,
    pub files: Vec<File>,
}
#[derive(Deserialize)]
struct Fonts {
    fonts: Vec<resources::Entry>,
}
pub fn fonts() -> Result<Vec<resources::Entry>> {
    Ok(serde_json::from_str::<Fonts>(include_str!("../../../../resources/fonts.lock.json"))?.fonts)
}
pub fn resources() -> Result<Vec<(String, resources::Entry)>> {
    let group: resources::Group =
        serde_json::from_str(include_str!("../../../../resources/tiger.lock.json"))?;
    let mut entries = Vec::new();
    for name in import::SOURCES {
        entries.push((
            format!("opt/tigertype/share/tigertype/tiger/{name}"),
            group
                .files
                .iter()
                .find(|e| &e.path == name)
                .ok_or("锁定码表项缺失")?
                .clone(),
        ));
    }
    for font in fonts()? {
        entries.push((
            format!("usr/share/fonts/truetype/tigertype/{}", font.path),
            font,
        ));
    }
    Ok(entries)
}
pub fn entry(path: &str, bytes: &[u8]) -> File {
    File {
        path: path.into(),
        size: bytes.len() as u64,
        sha256: resources::sha256_bytes(bytes),
    }
}
pub fn inspect(root: &Path) -> Result<Manifest> {
    let manifest: Manifest =
        serde_json::from_slice(&storage::read_file(&root.join("bundle.json"), 64 * 1024)?)?;
    if manifest.format != 1 || manifest.files.len() != 3 {
        return Err("安装包清单版本或数量不符".into());
    }
    let expected = BTreeSet::from(["tigertype.deb", "tigertype-fonts.deb", "tigertype-setup"]);
    let names: BTreeSet<_> = manifest.files.iter().map(|f| f.path.as_str()).collect();
    if names != expected {
        return Err("安装包文件集合不符".into());
    }
    for file in &manifest.files {
        read(root, file)?;
    }
    Ok(manifest)
}
pub fn read(root: &Path, file: &File) -> Result<Vec<u8>> {
    if file.size > 256 * 1024 * 1024 {
        return Err("安装包过大".into());
    }
    let bytes = storage::read_file(&root.join(resources::relative(&file.path)?), file.size)?;
    if bytes.len() as u64 != file.size || resources::sha256_bytes(&bytes) != file.sha256 {
        return Err(format!("安装包校验失败：{}", file.path).into());
    }
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gnome_adapter_is_part_of_the_managed_program_and_has_explicit_targets() {
        let metadata: serde_json::Value = serde_json::from_str(GNOME_FILES[0].1).unwrap();
        assert_eq!(metadata["uuid"], GNOME_UUID);
        assert_eq!(
            metadata["shell-version"],
            serde_json::json!(["46", "48", "50"])
        );
        assert_eq!(metadata["version"], 2);
        for (path, source) in GNOME_FILES {
            assert!(program_files().contains(&path));
            assert!(!source.is_empty());
        }
    }
    #[test]
    fn session_entries_are_recognized_across_versions() {
        let pending = session_entry(false);
        let steady = session_entry(true);
        assert!(pending.contains("login fcitx") && !steady.contains("login"));
        assert!(managed_session(pending.as_bytes()) && managed_session(steady.as_bytes()));
        // 旧版（v1，含 IBus 登录登记）也必须可被清理。
        assert!(managed_session(
            b"# TigerType managed session v1\n[Desktop Entry]\nExec=x login ibus\n"
        ));
        assert!(!managed_session(b"[Desktop Entry]\nExec=tigertype\n"));
    }
    #[test]
    fn target_is_explicit_and_downloads_use_official_https() {
        for (id, version) in [("ubuntu", "24.04"), ("ubuntu", "26.04"), ("debian", "13")] {
            let target = Target::parse(&format!("ID={id}\nVERSION_ID=\"{version}\"\n")).unwrap();
            assert!(target.sources().contains("https://"));
            assert!(!target.sources().contains("http://"));
        }
        for text in [
            "ID=ubuntu\nVERSION_ID=22.04",
            "ID=arch\nVERSION_ID=13",
            "ID=ubuntu\nID=debian\nVERSION_ID=13",
        ] {
            assert!(Target::parse(text).is_err());
        }
        assert_eq!(fonts().unwrap().len(), 6);
        assert_eq!(resources().unwrap().len(), import::SOURCES.len() + 6);
    }
    #[test]
    fn incomplete_or_changed_bundle_is_rejected_without_writing() {
        let root = resources::new_directory(
            &std::env::temp_dir().canonicalize().unwrap(),
            "tigertype-linux-bundle",
        )
        .unwrap();
        let mut manifest = Manifest {
            format: 1,
            target: Target {
                id: "ubuntu".into(),
                version: "26.04".into(),
            },
            files: Vec::new(),
        };
        for name in ["tigertype.deb", "tigertype-fonts.deb", "tigertype-setup"] {
            std::fs::write(root.join(name), name).unwrap();
            manifest.files.push(entry(name, name.as_bytes()));
        }
        std::fs::write(
            root.join("bundle.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(inspect(&root).is_ok());
        std::fs::write(root.join("tigertype.deb"), b"corrupt").unwrap();
        assert!(inspect(&root).is_err());
        assert_eq!(
            std::fs::read(root.join("tigertype-fonts.deb")).unwrap(),
            b"tigertype-fonts.deb"
        );
    }
}
