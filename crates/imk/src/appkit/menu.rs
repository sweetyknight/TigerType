//! 菜单目标由 Menu 持有，不使用全局回调表；旧菜单只能输出原控制器身份。
use super::*;
use crate::MenuAction;
use objc2::{runtime::AnyObject, sel};
use objc2_foundation::NSObject;
use tigertype_contracts::{Key, protocol::Panel};

struct TargetState {
    owner: u64,
    action: MenuAction,
    callback: Rc<dyn Fn(Action)>,
}
define_class!(
    // SAFETY: NSObject 主线程子类；action 签名与菜单 target/action 一致。
    #[unsafe(super = NSObject)]
    #[name = "TigerTypeInputMenuTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetState]
    struct MenuTarget;
    // SAFETY: 沿用 NSObject 身份与持有规则。
    unsafe impl NSObjectProtocol for MenuTarget {}
    impl MenuTarget {
        #[unsafe(method(invoke:))]
        fn invoke(&self, _: Option<&AnyObject>) { callback(|| {
            (self.ivars().callback)(Action::Menu { owner:self.ivars().owner, action:self.ivars().action });
        }); }
    }
);
/// 控制器持有 Menu 的时间必须覆盖系统使用 NSMenu 的时间；系统菜单不主动弹出、不进入嵌套循环。
pub struct Menu {
    menu: Retained<NSMenu>,
}
impl Menu {
    pub fn new(
        mtm: MainThreadMarker,
        owner: u64,
        state: &InputState,
        unavailable: Option<&str>,
        action: Rc<dyn Fn(Action)>,
    ) -> Self {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("TigerType 虎码"));
        menu.setAutoenablesItems(false);
        if let Some(reason) = unavailable {
            let item = NSMenuItem::new(mtm);
            item.setTitle(&NSString::from_str(reason));
            item.setEnabled(false);
            menu.addItem(&item);
        }
        let entries = [
            (
                if state.ascii_mode { "英文" } else { "中文" },
                MenuAction::Key(Key::ToggleAscii),
            ),
            (
                if state.full_shape { "全角" } else { "半角" },
                MenuAction::Key(Key::ToggleFullShape),
            ),
            (
                if state.ascii_punctuation {
                    "英文标点"
                } else {
                    "中文标点"
                },
                MenuAction::Key(Key::TogglePunctuation),
            ),
            ("查字", MenuAction::Show(Panel::Lookup)),
            ("设置", MenuAction::Show(Panel::Settings)),
        ];
        for (index, (label, command)) in entries.into_iter().enumerate() {
            if index == 3 {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            let this = MenuTarget::alloc(mtm).set_ivars(TargetState {
                owner,
                action: command,
                callback: action.clone(),
            });
            // SAFETY: init 初始化 NSObject；target 被 Menu 强持有，选择器签名对应 invoke:。
            unsafe {
                let target: Retained<MenuTarget> = msg_send![super(this), init];
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str(label));
                item.setTarget(Some(&target));
                // representedObject 强持有 target：调用方保留原生菜单后释放 Rust 包装也不会悬空。
                item.setRepresentedObject(Some(&target));
                item.setAction(Some(sel!(invoke:)));
                item.setEnabled(owner != 0 && (index >= 3 || unavailable.is_none()));
                menu.addItem(&item);
            }
        }
        Self { menu }
    }
    pub fn native(&self) -> &NSMenu {
        &self.menu
    }
}
