//! 安装预检的纯选择规则：优先现有框架，冲突或未声明的平台拒绝，不悄悄改用户输入环境。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framework {
    IBus,
    Fcitx5,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Gnome,
    Plasma,
}
pub fn desktop(value: &str) -> Result<Desktop, &'static str> {
    let parts: Vec<_> = value.split(':').map(str::to_ascii_lowercase).collect();
    match (
        parts.iter().any(|p| p == "gnome"),
        parts.iter().any(|p| p == "kde"),
    ) {
        (true, false) => Ok(Desktop::Gnome),
        (false, true) => Ok(Desktop::Plasma),
        _ => Err("当前目标仅包含 GNOME 或 KDE Plasma，其他桌面尚未支持"),
    }
}
pub fn framework(desktop: Desktop, ibus: bool, fcitx: bool) -> Result<Framework, &'static str> {
    match (ibus, fcitx) {
        (true, true) => Err("检测到两个活动输入法框架，不自动停用或替换任一个"),
        (true, false) => Ok(Framework::IBus),
        (false, true) => Ok(Framework::Fcitx5),
        (false, false) => Ok(match desktop {
            Desktop::Gnome => Framework::IBus,
            Desktop::Plasma => Framework::Fcitx5,
        }),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_defaults_preserve_existing_framework_and_reject_ambiguity() {
        assert_eq!(desktop("ubuntu:GNOME"), Ok(Desktop::Gnome));
        assert_eq!(desktop("KDE"), Ok(Desktop::Plasma));
        for name in ["", "GNOME:KDE", "XFCE", "notgnome"] {
            assert!(desktop(name).is_err());
        }
        for desk in [Desktop::Gnome, Desktop::Plasma] {
            assert_eq!(framework(desk, true, false), Ok(Framework::IBus));
            assert_eq!(framework(desk, false, true), Ok(Framework::Fcitx5));
            assert!(framework(desk, true, true).is_err());
        }
        assert_eq!(framework(Desktop::Gnome, false, false), Ok(Framework::IBus));
        assert_eq!(
            framework(Desktop::Plasma, false, false),
            Ok(Framework::Fcitx5)
        );
    }
}
