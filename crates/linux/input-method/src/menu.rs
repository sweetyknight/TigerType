//! 鼠标动作绑定展示编号，而不是点击到达时的候选序号；无编号的原生候选点击只能请求确认。
use std::collections::BTreeMap;
use tigertype_contracts::{InputState, Key, protocol::Panel};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Select { revision: u64, index: u8 },
    Key(Key),
    Show(Panel),
}
/// 五项基础菜单使用固定键：不提交文字、不绑定展示快照。GNOME 缓存首次注册的属性对象，
/// Fcitx5 的托盘菜单在焦点回到应用后才激活，两者都不能依赖每次重绘都变的名字。
pub const BASE: [(&str, Action); 5] = [
    ("tigertype-ascii", Action::Key(Key::ToggleAscii)),
    ("tigertype-shape", Action::Key(Key::ToggleFullShape)),
    ("tigertype-punctuation", Action::Key(Key::TogglePunctuation)),
    ("tigertype-lookup", Action::Show(Panel::Lookup)),
    ("tigertype-settings", Action::Show(Panel::Settings)),
];
/// 页脚说明（拼音反查标签与多页页码），两种框架共用同一文案。
pub fn caption(state: &InputState) -> String {
    tigertype_input_client::footer(state)
        .map(|footer| {
            format!(
                "{}{}",
                if footer.pinyin { "拼音 " } else { "" },
                footer
                    .pager
                    .map(|page| format!("第 {} 页", page.page))
                    .unwrap_or_default()
            )
        })
        .unwrap_or_default()
}
pub struct Menu {
    namespace: String,
    serial: u64,
    actions: BTreeMap<String, Action>,
    entries: Vec<(String, String)>,
}
impl Menu {
    pub fn new(namespace: String) -> Self {
        Self {
            namespace,
            serial: 0,
            actions: BTreeMap::new(),
            entries: Vec::new(),
        }
    }
    pub fn invalidate(&mut self) {
        self.actions.clear();
        self.entries.clear();
    }
    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }
    pub fn update(
        &mut self,
        state: &InputState,
        focused: bool,
        confirm: Option<usize>,
    ) -> Result<(), String> {
        self.invalidate();
        self.serial = self.serial.checked_add(1).ok_or("菜单展示编号耗尽")?;
        if !focused {
            return Ok(());
        }
        for (label, action) in [
            (
                if state.ascii_mode { "英文" } else { "中文" },
                Action::Key(Key::ToggleAscii),
            ),
            (
                if state.full_shape { "全角" } else { "半角" },
                Action::Key(Key::ToggleFullShape),
            ),
            (
                if state.ascii_punctuation {
                    "英文标点"
                } else {
                    "中文标点"
                },
                Action::Key(Key::TogglePunctuation),
            ),
            ("查字", Action::Show(Panel::Lookup)),
            ("设置", Action::Show(Panel::Settings)),
        ] {
            self.push(label.into(), action);
        }
        if state.composing {
            for (index, candidate) in state.candidates.iter().enumerate().take(9) {
                let label = if confirm == Some(index) {
                    format!("确认上屏：{}", candidate.text)
                } else {
                    format!("选字 {}：{}", index + 1, candidate.text)
                };
                self.push(
                    label,
                    Action::Select {
                        revision: state.revision,
                        index: index as u8,
                    },
                );
            }
            if state.page_index > 0 {
                self.push("上一页".into(), Action::Key(Key::PageUp));
            }
            if !state.is_last_page {
                self.push("下一页".into(), Action::Key(Key::PageDown));
            }
            self.push("取消组合".into(), Action::Key(Key::Escape));
        }
        Ok(())
    }
    fn push(&mut self, label: String, action: Action) {
        let name = format!("{}/{}/{}", self.namespace, self.serial, self.entries.len());
        self.actions.insert(name.clone(), action);
        self.entries.push((name, label));
    }
    pub fn take(&mut self, name: &str) -> Option<Action> {
        let action = self.actions.get(name).copied()?;
        // 有效动作一次性消费整次展示，重复投递和旧菜单都不能操作新组合。
        self.invalidate();
        Some(action)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tigertype_contracts::Candidate;
    #[test]
    fn menu_names_bind_snapshot_focus_and_instance_and_are_single_use() {
        let mut menu = Menu::new("connection/engine1".into());
        let mut state = InputState {
            composing: true,
            revision: 7,
            candidates: vec![Candidate {
                text: "虎".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        menu.update(&state, true, None).unwrap();
        let old = menu.entries()[5].0.clone();
        state.revision = 8;
        menu.update(&state, true, Some(0)).unwrap();
        assert_eq!(menu.take(&old), None);
        assert_eq!(menu.entries()[5].1, "确认上屏：虎");
        let current = menu.entries()[5].0.clone();
        assert_eq!(
            menu.take(&current),
            Some(Action::Select {
                revision: 8,
                index: 0
            })
        );
        assert_eq!(menu.take(&current), None);
        menu.update(&state, true, None).unwrap();
        let current = menu.entries()[5].0.clone();
        menu.update(&state, false, None).unwrap();
        assert_eq!(menu.take(&current), None);
        let mut another = Menu::new("connection/engine2".into());
        another.update(&state, true, None).unwrap();
        assert_eq!(another.take(&old), None);
    }
}
