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
/// 参数为两个框架总线名的当前所有者；Fcitx5 的 IBus 兼容前端会同时持有两个名字，同一所有者只算 Fcitx5。
pub fn framework(
    desktop: Desktop,
    ibus: Option<&str>,
    fcitx: Option<&str>,
) -> Result<Framework, &'static str> {
    match (ibus.is_some() && ibus != fcitx, fcitx.is_some()) {
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
            assert_eq!(framework(desk, Some(":1.2"), None), Ok(Framework::IBus));
            assert_eq!(framework(desk, None, Some(":1.3")), Ok(Framework::Fcitx5));
            assert!(framework(desk, Some(":1.2"), Some(":1.3")).is_err());
            // Fcitx5 兼容前端同时持有 IBus 名字时不是两个框架。
            assert_eq!(
                framework(desk, Some(":1.3"), Some(":1.3")),
                Ok(Framework::Fcitx5)
            );
        }
        assert_eq!(framework(Desktop::Gnome, None, None), Ok(Framework::IBus));
        assert_eq!(
            framework(Desktop::Plasma, None, None),
            Ok(Framework::Fcitx5)
        );
    }
}
