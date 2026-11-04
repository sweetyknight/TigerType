//! IBus D-Bus 边界：校验私有总线的 daemon 身份，按键共用 50ms 期限，回调 panic 不跨 GIO。
use crate::{
    Keyboard,
    menu::{Action, Menu},
};
use gio::{DBusConnection, DBusNodeInfo, RegistrationId, glib};
use glib::{Variant, variant::ToVariant};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    time::{Duration, Instant},
};
use tigertype_contracts::{CandidateFont, InputState, Key, KeyOutcome, protocol::Panel};
use tigertype_input_client::{Channel, Host};

const ENGINE: &str = "org.freedesktop.IBus.Engine";
const FACTORY: &str = "org.freedesktop.IBus.Factory";
// GNOME 缓存首次注册的属性对象；这五项不提交文字，使用固定键并以 UpdateProperty 更新标签。
const BASE_PROPERTIES: [(&str, Action); 5] = [
    ("tigertype-ascii", Action::Key(Key::ToggleAscii)),
    ("tigertype-shape", Action::Key(Key::ToggleFullShape)),
    ("tigertype-punctuation", Action::Key(Key::TogglePunctuation)),
    ("tigertype-lookup", Action::Show(Panel::Lookup)),
    ("tigertype-settings", Action::Show(Panel::Settings)),
];
fn authorized(sender: Option<&str>, daemon: &str) -> bool {
    // IBus 私有总线把内部调用标为保留名，并覆盖外部连接的 sender；客户端不能伪造此名。
    sender == Some(daemon) || sender == Some("org.freedesktop.DBus")
}
const XML: &str = r#"<node>
<interface name="org.freedesktop.IBus.Factory"><method name="CreateEngine"><arg type="s" direction="in"/><arg type="o" direction="out"/></method></interface>
<interface name="org.freedesktop.IBus.Engine">
<method name="ProcessKeyEvent"><arg type="u" direction="in"/><arg type="u" direction="in"/><arg type="u" direction="in"/><arg type="b" direction="out"/></method>
<method name="FocusIn"/><method name="FocusOut"/>
<method name="FocusInId"><arg type="s" direction="in"/><arg type="s" direction="in"/></method>
<method name="FocusOutId"><arg type="s" direction="in"/></method>
<method name="Enable"/><method name="Disable"/><method name="Reset"/>
<method name="SetCapabilities"><arg type="u" direction="in"/></method>
<method name="SetCursorLocation"><arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
<method name="SetSurroundingText"><arg type="v" direction="in"/><arg type="u" direction="in"/><arg type="u" direction="in"/></method>
<method name="PageUp"/><method name="PageDown"/><method name="CursorUp"/><method name="CursorDown"/>
<method name="CandidateClicked"><arg type="u" direction="in"/><arg type="u" direction="in"/><arg type="u" direction="in"/></method>
<method name="PropertyActivate"><arg type="s" direction="in"/><arg type="u" direction="in"/></method>
<method name="PropertyShow"><arg type="s" direction="in"/></method><method name="PropertyHide"><arg type="s" direction="in"/></method>
<signal name="CommitText"><arg type="v"/></signal>
<signal name="UpdatePreeditText"><arg type="v"/><arg type="u"/><arg type="b"/><arg type="u"/></signal>
<signal name="UpdateLookupTable"><arg type="v"/><arg type="b"/></signal>
<signal name="UpdateAuxiliaryText"><arg type="v"/><arg type="b"/></signal>
<signal name="RegisterProperties"><arg type="v"/></signal>
<signal name="UpdateProperty"><arg type="v"/></signal>
<property name="FocusId" type="(b)" access="read"/><property name="ActiveSurroundingText" type="(b)" access="read"/><property name="ContentType" type="(uu)" access="write"/>
</interface>
<interface name="org.freedesktop.IBus.Service"><method name="Destroy"/></interface>
</node>"#;

fn attachments() -> BTreeMap<String, Variant> {
    BTreeMap::new()
}
fn text(value: &str) -> Variant {
    // IBusSerializable 首两项固定为类型名与附件；空属性列表不能省略或使用空字符串替代。
    let attributes = ("IBusAttrList", attachments(), Vec::<Variant>::new()).to_variant();
    ("IBusText", attachments(), value, attributes).to_variant()
}
fn lookup(state: &InputState) -> Variant {
    // 附件仅描述本次展示；Shell 适配层不读配置/词表，也不据此生成或提交候选。
    let p = state.options.palette();
    let style = (
        1u32,
        match state.options.candidate_font {
            CandidateFont::WenKai => 0u32,
            CandidateFont::KaiTi => 1u32,
        },
        vec![
            p.window,
            p.border,
            p.text,
            p.label,
            p.muted,
            p.highlight,
            p.highlight_text,
            p.chosen,
        ],
    )
        .to_variant();
    // IBus 的历史附件契约在 a{sv} 内再包一层 v，与普通 D-Bus 字典不同。
    let metadata = BTreeMap::from([("tigertype-style".to_owned(), style.to_variant())]);
    let candidates: Vec<_> = state
        .candidates
        .iter()
        .map(|candidate| {
            let plain = if candidate.comment.is_empty() {
                candidate.text.clone()
            } else {
                format!("{}  {}", candidate.text, candidate.comment)
            };
            let parts = BTreeMap::from([(
                "tigertype-candidate".to_owned(),
                (&candidate.text, &candidate.comment)
                    .to_variant()
                    .to_variant(),
            )]);
            (
                "IBusText",
                parts,
                plain,
                ("IBusAttrList", attachments(), Vec::<Variant>::new()).to_variant(),
            )
                .to_variant()
        })
        .collect();
    let labels: Vec<_> = state
        .candidates
        .iter()
        .map(|candidate| text(&candidate.label))
        .collect();
    (
        "IBusLookupTable",
        metadata,
        state.options.page_size as u32,
        state.highlighted.unwrap_or(0) as u32,
        true,
        false,
        1_i32,
        candidates,
        labels,
    )
        .to_variant()
}

struct Engine {
    host: Host<Box<dyn Channel>>,
    keyboard: Keyboard,
    focused: bool,
    protected: bool,
    menu: Menu,
    base_labels: [String; 5],
}
impl Engine {
    fn clear(&mut self) {
        self.host.discard();
        self.host.blur();
        self.keyboard.reset();
        self.menu.invalidate();
    }
}
struct Entry {
    engine: Rc<RefCell<Engine>>,
    registrations: Vec<RegistrationId>,
}
type Engines = Rc<RefCell<BTreeMap<String, Entry>>>;
pub struct Server {
    connection: DBusConnection,
    factory: Option<RegistrationId>,
    engines: Engines,
}
impl Server {
    /// connection 是启动期建立的 IBus 私有连接，daemon 必须为该连接确认的唯一名称。
    pub fn new(
        connection: DBusConnection,
        daemon: String,
        channel: impl Fn() -> Box<dyn Channel> + 'static,
        show: impl Fn(Panel) + 'static,
    ) -> Result<Self, String> {
        if !daemon.starts_with(':') || daemon.len() > 255 {
            return Err("IBus daemon 名称无效".into());
        }
        let interfaces = DBusNodeInfo::for_xml(XML).map_err(|e| e.to_string())?;
        let engines: Engines = Rc::new(RefCell::new(BTreeMap::new()));
        let weak = Rc::downgrade(&engines);
        let last = Cell::new(0_u64);
        let show: Rc<dyn Fn(Panel)> = Rc::new(show);
        let factory_info = interfaces.lookup_interface(FACTORY).ok_or("工厂接口缺失")?;
        let factory = connection
            .register_object("/org/freedesktop/IBus/Factory", &factory_info)
            .method_call(move |connection, sender, _, _, _, parameters, invocation| {
                let result = catch_unwind(AssertUnwindSafe(|| -> Result<Variant, String> {
                    if !authorized(sender, &daemon) {
                        return Err("拒绝非 IBus daemon 调用".into());
                    }
                    if parameters.size() > 1024 {
                        return Err("工厂请求长度超限".into());
                    }
                    if parameters.get::<(String,)>().as_ref().map(|p| p.0.as_str())
                        != Some("tigertype")
                    {
                        return Err("未知输入法名称".into());
                    }
                    let engines = weak.upgrade().ok_or("输入法已退出")?;
                    if engines.borrow().len() >= 16 {
                        return Err("输入法实例数超限".into());
                    }
                    let next = last.get().checked_add(1).ok_or("实例编号耗尽")?;
                    last.set(next);
                    let path = format!("/org/freedesktop/IBus/Engine/TigerType/{next}");
                    let engine = Rc::new(RefCell::new(Engine {
                        host: Host::new(channel()),
                        keyboard: Keyboard::default(),
                        focused: false,
                        protected: false,
                        menu: Menu::new(format!(
                            "{}{path}",
                            connection.unique_name().ok_or("缺少总线身份")?
                        )),
                        base_labels: std::array::from_fn(|_| String::new()),
                    }));
                    let mut registrations = Vec::new();
                    for interface in [ENGINE, "org.freedesktop.IBus.Service"] {
                        let info = interfaces
                            .lookup_interface(interface)
                            .ok_or("引擎接口缺失")?;
                        let state = engine.clone();
                        let owner = if sender == Some("org.freedesktop.DBus") {
                            "org.freedesktop.IBus".into()
                        } else {
                            daemon.clone()
                        };
                        let show = show.clone();
                        let weak = Rc::downgrade(&engines);
                        let property_state = engine.clone();
                        let property_owner = owner.clone();
                        let registration = connection
                            .register_object(&path, &info)
                            .property(|_, _, _, _, name| (name == "FocusId",).to_variant())
                            .set_property(move |bus, sender, path, _, name, value| {
                                catch_unwind(AssertUnwindSafe(|| {
                                    if !authorized(sender, &property_owner) || name != "ContentType"
                                    {
                                        return false;
                                    }
                                    let Some((purpose, _)) = value.get::<(u32, u32)>() else {
                                        return false;
                                    };
                                    let Ok(mut state) = property_state.try_borrow_mut() else {
                                        return false;
                                    };
                                    state.protected = matches!(purpose, 8 | 9);
                                    if state.protected {
                                        state.clear();
                                        let _ = clear_panel(&bus, &property_owner, path);
                                    }
                                    true
                                }))
                                .unwrap_or(false)
                            })
                            .method_call(
                                move |bus, sender, path, _, method, parameters, invocation| {
                                    if !authorized(sender, &owner) {
                                        invocation.return_dbus_error(
                                            "org.freedesktop.DBus.Error.AccessDenied",
                                            "拒绝非 IBus daemon 调用",
                                        );
                                        return;
                                    }
                                    // 所有错误路径都回复；panic 后关闭旧会话，不能让下一键复用不确定结果。
                                    let result = catch_unwind(AssertUnwindSafe(|| {
                                        // 在复制字符串或解释周边文字之前限制大小；不读取或保留宿主的大段文档。
                                        if parameters.size() > tigertype_contracts::MAX_TEXT_BYTES {
                                            return Err("输入法请求长度超限".into());
                                        }
                                        handle(
                                            &mut state.borrow_mut(),
                                            &bus,
                                            &owner,
                                            path,
                                            method,
                                            &parameters,
                                            &*show,
                                        )
                                    }))
                                    .unwrap_or_else(|_| {
                                        state.borrow_mut().host.close();
                                        Err("输入法回调失败".into())
                                    });
                                    if method == "Destroy"
                                        && let Some(engines) = weak.upgrade()
                                        && let Some(entry) = engines.borrow_mut().remove(path)
                                    {
                                        for id in entry.registrations {
                                            let _ = bus.unregister_object(id);
                                        }
                                    }
                                    match result {
                                        Ok(result) => invocation.return_value(Some(&result)),
                                        Err(_) if method == "ProcessKeyEvent" => {
                                            state.borrow_mut().host.close();
                                            state.borrow_mut().menu.invalidate();
                                            let _ = clear_panel(&bus, &owner, path);
                                            invocation.return_value(Some(&(false,).to_variant()));
                                        }
                                        Err(error) => invocation.return_dbus_error(
                                            "org.freedesktop.DBus.Error.Failed",
                                            &error,
                                        ),
                                    }
                                },
                            )
                            .build();
                        match registration {
                            Ok(id) => registrations.push(id),
                            Err(error) => {
                                for id in registrations {
                                    let _ = connection.unregister_object(id);
                                }
                                return Err(error.to_string());
                            }
                        }
                    }
                    engines.borrow_mut().insert(
                        path.clone(),
                        Entry {
                            engine,
                            registrations,
                        },
                    );
                    Ok(
                        (glib::variant::ObjectPath::try_from(path).map_err(|e| e.to_string())?,)
                            .to_variant(),
                    )
                }))
                .unwrap_or_else(|_| Err("创建输入法实例失败".into()));
                match result {
                    Ok(result) => invocation.return_value(Some(&result)),
                    Err(error) => {
                        invocation.return_dbus_error("org.freedesktop.DBus.Error.Failed", &error)
                    }
                }
            })
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            connection,
            factory: Some(factory),
            engines,
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(id) = self.factory.take() {
            let _ = self.connection.unregister_object(id);
        }
        for (_, entry) in std::mem::take(&mut *self.engines.borrow_mut()) {
            entry.engine.borrow_mut().host.close();
            for id in entry.registrations {
                let _ = self.connection.unregister_object(id);
            }
        }
    }
}
fn signal(
    bus: &DBusConnection,
    owner: &str,
    path: &str,
    name: &str,
    value: Variant,
) -> Result<(), String> {
    // 原生 IBus 引擎信号按官方接口广播，由 daemon 的引擎代理订阅；定向到服务名会被私有总线路由丢弃。
    let destination = if owner == "org.freedesktop.IBus" {
        None
    } else {
        Some(owner)
    };
    bus.emit_signal(destination, path, ENGINE, name, Some(&value))
        .map_err(|e| e.to_string())
}
fn clear_panel(bus: &DBusConnection, owner: &str, path: &str) -> Result<(), String> {
    signal(
        bus,
        owner,
        path,
        "RegisterProperties",
        (properties(&[]),).to_variant(),
    )?;
    // mode=0 要求失焦丢弃预编辑，不能把尚未选定的编码隐式上屏。
    signal(
        bus,
        owner,
        path,
        "UpdatePreeditText",
        (text(""), 0_u32, false, 0_u32).to_variant(),
    )?;
    signal(
        bus,
        owner,
        path,
        "UpdateLookupTable",
        (lookup(&InputState::default()), false).to_variant(),
    )?;
    signal(
        bus,
        owner,
        path,
        "UpdateAuxiliaryText",
        (text(""), false).to_variant(),
    )
}
fn property(key: &str, label: &str) -> Variant {
    // IBusProperty 的字段顺序与官方可序列化协议一致，包括末尾 symbol，不能省略空子菜单。
    (
        "IBusProperty",
        attachments(),
        key,
        0_u32,
        text(label),
        "",
        text(label),
        true,
        true,
        0_u32,
        ("IBusPropList", attachments(), Vec::<Variant>::new()).to_variant(),
        text(""),
    )
        .to_variant()
}
fn properties(entries: &[(String, String)]) -> Variant {
    let props: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(index, (key, label))| {
            property(
                BASE_PROPERTIES
                    .get(index)
                    .map_or(key.as_str(), |entry| entry.0),
                label,
            )
        })
        .collect();
    ("IBusPropList", attachments(), props).to_variant()
}
fn publish_menu(
    engine: &mut Engine,
    bus: &DBusConnection,
    owner: &str,
    path: &str,
    confirm: Option<usize>,
) -> Result<(), String> {
    engine.menu.update(
        engine.host.state(),
        engine.focused && !engine.protected,
        confirm,
    )?;
    signal(
        bus,
        owner,
        path,
        "RegisterProperties",
        (properties(engine.menu.entries()),).to_variant(),
    )?;
    for (index, (_, label)) in engine
        .menu
        .entries()
        .iter()
        .take(BASE_PROPERTIES.len())
        .enumerate()
    {
        if engine.base_labels[index] != *label {
            signal(
                bus,
                owner,
                path,
                "UpdateProperty",
                (property(BASE_PROPERTIES[index].0, label),).to_variant(),
            )?;
            engine.base_labels[index].clone_from(label);
        }
    }
    Ok(())
}
fn publish(
    bus: &DBusConnection,
    owner: &str,
    path: &str,
    outcome: KeyOutcome,
    deadline: Instant,
) -> Result<(bool, bool), String> {
    let state = &outcome.state;
    let cursor = state
        .composition
        .text
        .get(..state.composition.cursor)
        .ok_or("组合光标不是字符边界")?
        .chars()
        .count() as u32;
    let preedit = (
        text(&state.composition.text),
        cursor,
        state.composing,
        0_u32,
    )
        .to_variant();
    let table = (
        lookup(state),
        state.composing && !state.candidates.is_empty(),
    )
        .to_variant();
    let caption = tigertype_input_client::footer(state)
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
        .unwrap_or_default();
    let auxiliary = (text(&caption), !caption.is_empty()).to_variant();
    let commit = outcome
        .commit
        .as_deref()
        .map(|value| (text(value),).to_variant());
    if Instant::now() >= deadline {
        return Err("按键期限已过，不发送迟到提交".into());
    }
    if let Some(commit) = commit {
        clear_panel(bus, owner, path)?;
        if Instant::now() >= deadline {
            return Err("撤下旧组合后期限已过，不发送提交".into());
        }
        signal(bus, owner, path, "CommitText", commit)?;
        // 提交已交给框架后不能再放行原键，否则会重复输入；后续展示失败只撤下未上屏组合。
        if signal(bus, owner, path, "UpdatePreeditText", preedit).is_err()
            || signal(bus, owner, path, "UpdateLookupTable", table).is_err()
            || signal(bus, owner, path, "UpdateAuxiliaryText", auxiliary).is_err()
        {
            let _ = clear_panel(bus, owner, path);
            return Ok((true, false));
        }
        return Ok((true, true));
    }
    signal(bus, owner, path, "UpdatePreeditText", preedit)?;
    signal(bus, owner, path, "UpdateLookupTable", table)?;
    signal(bus, owner, path, "UpdateAuxiliaryText", auxiliary)?;
    Ok((outcome.handled, true))
}
fn handle(
    engine: &mut Engine,
    bus: &DBusConnection,
    owner: &str,
    path: &str,
    method: &str,
    parameters: &Variant,
    show: &dyn Fn(Panel),
) -> Result<Variant, String> {
    let deadline = Instant::now() + Duration::from_millis(50);
    let key = match method {
        "ProcessKeyEvent" => {
            let (symbol, _, state) = parameters.get::<(u32, u32, u32)>().ok_or("按键参数不符")?;
            if !engine.focused || engine.protected {
                return Ok((false,).to_variant());
            }
            engine
                .keyboard
                .key(symbol, state, engine.host.state().composing)
        }
        "FocusIn" | "FocusInId" => {
            engine.clear();
            engine.focused = true;
            clear_panel(bus, owner, path)?;
            publish_menu(engine, bus, owner, path, None)?;
            return Ok(().to_variant());
        }
        "FocusOut" | "FocusOutId" | "Disable" | "Destroy" => {
            engine.clear();
            engine.focused = false;
            if method == "Destroy" {
                engine.host.close();
            }
            clear_panel(bus, owner, path)?;
            return Ok(().to_variant());
        }
        "Reset" => {
            engine.clear();
            clear_panel(bus, owner, path)?;
            return Ok(().to_variant());
        }
        "CandidateClicked" => {
            let (index, button, _) = parameters.get::<(u32, u32, u32)>().ok_or("点击参数不符")?;
            if !engine.focused
                || engine.protected
                || !engine.host.state().composing
                || button != 1
                || index as usize >= engine.host.state().candidates.len()
            {
                return Err("候选点击已失效".into());
            }
            // 原生点击没有快照：只显示明确文字的确认项，第二次版本化菜单动作才允许提交。
            publish_menu(engine, bus, owner, path, Some(index as usize))?;
            signal(
                bus,
                owner,
                path,
                "UpdateAuxiliaryText",
                (text("请在输入法菜单确认要上屏的文字"), true).to_variant(),
            )?;
            return Ok(().to_variant());
        }
        "PageUp" | "PageDown" | "CursorUp" | "CursorDown" => {
            return Err("请使用输入法菜单中的版本化翻页项或键盘翻页".into());
        }
        "PropertyActivate" => {
            let (property, _) = parameters.get::<(String, u32)>().ok_or("菜单参数不符")?;
            let base = BASE_PROPERTIES
                .iter()
                .find(|entry| entry.0 == property)
                .map(|entry| entry.1);
            // 打开独立窗口不操作文档，允许系统菜单导致的 Reset/失焦；不借此放宽选字校验。
            if let Some(Action::Show(panel)) = base {
                show(panel);
                return Ok(().to_variant());
            }
            if !engine.focused || engine.protected {
                return Err("菜单焦点已失效".into());
            }
            let action = if let Some(action) = base {
                // 固定模式命令只允许空组合；不能把迟到的切换动作变成新组合的原码提交。
                if engine.host.state().composing {
                    return Err("请先结束组合再切换模式".into());
                }
                action
            } else {
                engine.menu.take(&property).ok_or("菜单快照已失效")?
            };
            let outcome = match action {
                Action::Show(panel) => {
                    show(panel);
                    publish_menu(engine, bus, owner, path, None)?;
                    return Ok(().to_variant());
                }
                Action::Select { revision, index } => engine
                    .host
                    .select(revision, index, deadline)
                    .map_err(|_| "选字通道失败")?,
                Action::Key(key) => {
                    Some(engine.host.key(key, deadline).map_err(|_| "菜单通道失败")?)
                }
            };
            if let Some(outcome) = outcome {
                let (_, shown) = publish(bus, owner, path, outcome, deadline)?;
                if !shown {
                    engine.clear();
                }
            }
            publish_menu(engine, bus, owner, path, None)?;
            return Ok(().to_variant());
        }
        "Enable" | "SetCapabilities" | "SetCursorLocation" | "SetSurroundingText"
        | "PropertyShow" | "PropertyHide" => return Ok(().to_variant()),
        _ => return Err("不支持的输入法方法".into()),
    };
    let Some(key) = key else {
        if method == "ProcessKeyEvent" {
            let (symbol, _, state) = parameters.get::<(u32, u32, u32)>().ok_or("按键参数不符")?;
            // 放行可能移动光标的未知键或应用快捷键前先撤销组合；普通抬键和修饰键不影响组合。
            if state & (1 << 30) == 0
                && !(0xffe1..=0xffee).contains(&symbol)
                && engine.host.state().composing
            {
                engine.clear();
                clear_panel(bus, owner, path)?;
            }
        }
        return Ok((false,).to_variant());
    };
    if !engine.host.ready(deadline) {
        clear_panel(bus, owner, path)?;
        return Ok((false,).to_variant());
    }
    // ready 可能重建会话；旧组合下识别的 Ctrl+L 不能在新空会话里吞掉应用快捷键。
    if key == Key::ToggleLearning && !engine.host.state().composing {
        clear_panel(bus, owner, path)?;
        return Ok((false,).to_variant());
    }
    if !engine.host.state().consumes(key) {
        return Ok((false,).to_variant());
    }
    let outcome = engine
        .host
        .key(key, deadline)
        .map_err(|_| "引擎不可用，放行原键")?;
    let (handled, shown) = publish(bus, owner, path, outcome, deadline)?;
    if !shown {
        engine.clear();
    }
    // 提交已经发送时，菜单显示失败不能让调用者再放行同一原键。
    if publish_menu(engine, bus, owner, path, None).is_err() {
        engine.menu.invalidate();
    }
    Ok((handled,).to_variant())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        process::{Child, Command, Stdio},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };
    use tigertype_contracts::{
        Candidate, Composition, Key,
        protocol::{Command as InputCommand, Reply},
    };
    use tigertype_input_client::Error;

    struct Daemon(Child);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    struct Quit(glib::MainLoop);
    impl Drop for Quit {
        fn drop(&mut self) {
            self.0.quit();
        }
    }
    struct Mock {
        fresh: bool,
        calls: Arc<AtomicUsize>,
        reconnect: Arc<AtomicBool>,
    }
    impl Channel for Mock {
        fn ensure(&mut self, _: Instant) -> Result<bool, Error> {
            let reconnect = self.reconnect.swap(false, Ordering::SeqCst);
            Ok(std::mem::replace(&mut self.fresh, false) || reconnect)
        }
        fn request(&mut self, _: u64, command: InputCommand, _: Instant) -> Result<Reply, Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let InputCommand::Select { index: 0, .. } = command {
                return Ok(Reply::Key(KeyOutcome {
                    handled: true,
                    commit: Some("虎".into()),
                    state: InputState::default(),
                }));
            }
            let InputCommand::Key { key, .. } = command else {
                return Err(Error::Rejected);
            };
            let mut state = InputState::default();
            let commit = match key {
                Key::Character('z') => {
                    // 故意模拟不遵守期限的通道：边界仍须拒绝它返回的迟到提交。
                    std::thread::sleep(Duration::from_millis(65));
                    Some("迟到".into())
                }
                Key::Space => Some("虎".into()),
                _ => {
                    state.composition = Composition {
                        text: "a".into(),
                        cursor: 1,
                        selection: 1..1,
                    };
                    state.composing = true;
                    state.candidates = vec![Candidate {
                        text: "虎".into(),
                        label: "1".into(),
                        ..Default::default()
                    }];
                    state.highlighted = Some(0);
                    None
                }
            };
            Ok(Reply::Key(KeyOutcome {
                handled: true,
                commit,
                state,
            }))
        }
        fn show(&mut self, _: Panel, _: Instant) -> Result<(), Error> {
            Ok(())
        }
        fn indicate(&mut self, _: bool, _: Instant) -> Result<(), Error> {
            Ok(())
        }
        fn cancel(&mut self) {
            self.fresh = true;
        }
    }
    fn connect(address: &str) -> DBusConnection {
        DBusConnection::for_address_sync(
            address,
            gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
                | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
            None,
            gio::Cancellable::NONE,
        )
        .unwrap()
    }
    #[test]
    fn private_bus_checks_identity_focus_deadline_and_destroys_objects() {
        // 独立消息总线只存在于此测试进程树，不修改用户会话、输入源或个人配置。
        let mut daemon = Daemon(
            Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut address = String::new();
        BufReader::new(daemon.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                let client = connect(&address);
                let bus = connect(&address);
                let service = bus.unique_name().unwrap().to_string();
                let owner = client.unique_name().unwrap().to_string();
                let calls = Arc::new(AtomicUsize::new(0));
                let count = calls.clone();
                let panels = Arc::new(Mutex::new(Vec::new()));
                let shown_panels = panels.clone();
                let reconnect = Arc::new(AtomicBool::new(false));
                let reset = reconnect.clone();
                let server = Server::new(
                    bus,
                    owner,
                    move || {
                        Box::new(Mock {
                            fresh: true,
                            calls: count.clone(),
                            reconnect: reset.clone(),
                        })
                    },
                    move |panel| shown_panels.lock().unwrap().push(panel),
                )
                .unwrap();
                let main = glib::MainLoop::new(Some(&context), false);
                let quit = main.clone();
                let worker = std::thread::spawn(move || {
                    let _quit = Quit(quit);
                    let commits = Arc::new(Mutex::new(Vec::new()));
                    let collected = commits.clone();
                    let menus = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
                    let captured = menus.clone();
                    let filter = client.add_filter(move |_, message, incoming| {
                        if incoming
                            && message.member().as_deref() == Some("RegisterProperties")
                            && let Some(body) = message.body()
                            && let Some(props) = body.child_value(0).as_variant()
                            && let Some(entries) = props.child_value(2).get::<Vec<Variant>>()
                            && let Ok(mut captured) = captured.lock()
                        {
                            *captured = entries
                                .iter()
                                .filter_map(|entry| {
                                    let key = entry.child_value(2).get::<String>()?;
                                    let label = entry
                                        .child_value(4)
                                        .as_variant()?
                                        .child_value(2)
                                        .get::<String>()?;
                                    Some((key, label))
                                })
                                .collect();
                        }
                        if incoming
                            && message.member().as_deref() == Some("CommitText")
                            && let Some(body) = message.body()
                            && let Ok(mut collected) = collected.lock()
                        {
                            collected.push(body);
                        }
                        Some(message.clone())
                    });
                    let call = |connection: &DBusConnection,
                                path: &str,
                                interface: &str,
                                method: &str,
                                value: Variant| {
                        connection.call_sync(
                            Some(&service),
                            path,
                            interface,
                            method,
                            Some(&value),
                            None,
                            gio::DBusCallFlags::NONE,
                            2000,
                            gio::Cancellable::NONE,
                        )
                    };
                    let stranger = connect(&address);
                    assert!(
                        call(
                            &stranger,
                            "/org/freedesktop/IBus/Factory",
                            FACTORY,
                            "CreateEngine",
                            ("tigertype",).to_variant()
                        )
                        .is_err()
                    );
                    let reply = call(
                        &client,
                        "/org/freedesktop/IBus/Factory",
                        FACTORY,
                        "CreateEngine",
                        ("tigertype",).to_variant(),
                    )
                    .unwrap();
                    let (path,) = reply.get::<(glib::variant::ObjectPath,)>().unwrap();
                    let path = path.as_str();
                    assert!(
                        call(
                            &client,
                            "/org/freedesktop/IBus/Factory",
                            FACTORY,
                            "CreateEngine",
                            ("a".repeat(2048),).to_variant()
                        )
                        .unwrap_err()
                        .to_string()
                        .contains("长度超限")
                    );
                    call(
                        &client,
                        path,
                        ENGINE,
                        "PropertyShow",
                        ("settings",).to_variant(),
                    )
                    .unwrap();
                    assert!(
                        call(
                            &client,
                            path,
                            ENGINE,
                            "PropertyShow",
                            ("a".repeat(tigertype_contracts::MAX_TEXT_BYTES + 1),).to_variant()
                        )
                        .unwrap_err()
                        .to_string()
                        .contains("长度超限")
                    );
                    let key = |symbol: u32| {
                        call(
                            &client,
                            path,
                            ENGINE,
                            "ProcessKeyEvent",
                            (symbol, 0_u32, 0_u32).to_variant(),
                        )
                        .unwrap()
                        .get::<(bool,)>()
                        .unwrap()
                        .0
                    };
                    assert!(!key(u32::from(b'a')), "未聚焦不能吃键");
                    call(&client, path, ENGINE, "FocusIn", ().to_variant()).unwrap();
                    assert!(
                        call(
                            &stranger,
                            path,
                            ENGINE,
                            "ProcessKeyEvent",
                            (97_u32, 0_u32, 0_u32).to_variant()
                        )
                        .is_err()
                    );
                    assert_eq!(calls.load(Ordering::SeqCst), 0);
                    assert!(key(u32::from(b'a')));
                    let before = calls.load(Ordering::SeqCst);
                    assert!(
                        call(
                            &client,
                            path,
                            ENGINE,
                            "PropertyActivate",
                            ("tigertype-ascii", 0u32).to_variant()
                        )
                        .is_err(),
                        "固定模式命令不能提交正在输入的新组合"
                    );
                    assert_eq!(calls.load(Ordering::SeqCst), before);
                    reconnect.store(true, Ordering::SeqCst);
                    assert_eq!(
                        call(
                            &client,
                            path,
                            ENGINE,
                            "ProcessKeyEvent",
                            (u32::from(b'l'), 0_u32, 1_u32 << 2).to_variant()
                        )
                        .unwrap()
                        .get::<(bool,)>(),
                        Some((false,)),
                        "会话重建后 Ctrl+L 必须放行"
                    );
                    assert_eq!(calls.load(Ordering::SeqCst), 1);
                    assert!(key(u32::from(b'a')));
                    assert!(key(0x20));
                    assert_eq!(commits.lock().unwrap().len(), 1);
                    assert!(!key(u32::from(b'z')), "超时返回的提交必须丢弃");
                    assert_eq!(commits.lock().unwrap().len(), 1);
                    assert!(
                        call(
                            &client,
                            path,
                            ENGINE,
                            "CandidateClicked",
                            (0_u32, 1_u32, 0_u32).to_variant()
                        )
                        .is_err()
                    );
                    assert!(key(u32::from(b'a')));
                    assert!(!key(0xff50), "Home 放行前撤销组合");
                    assert!(key(u32::from(b'a')));
                    let old = menus
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|(_, label)| label == "选字 1：虎")
                        .unwrap()
                        .0
                        .clone();
                    call(
                        &client,
                        path,
                        ENGINE,
                        "CandidateClicked",
                        (0u32, 1u32, 0u32).to_variant(),
                    )
                    .unwrap();
                    assert_eq!(
                        commits.lock().unwrap().len(),
                        1,
                        "原生序号点击只能提出确认，不能直接上屏"
                    );
                    let token = menus
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|(_, label)| label == "确认上屏：虎")
                        .unwrap()
                        .0
                        .clone();
                    assert!(
                        call(
                            &client,
                            path,
                            ENGINE,
                            "PropertyActivate",
                            (&old, 0u32).to_variant()
                        )
                        .is_err()
                    );
                    call(
                        &client,
                        path,
                        ENGINE,
                        "PropertyActivate",
                        (&token, 0u32).to_variant(),
                    )
                    .unwrap();
                    assert_eq!(commits.lock().unwrap().len(), 2);
                    assert!(
                        call(
                            &client,
                            path,
                            ENGINE,
                            "PropertyActivate",
                            (&token, 0u32).to_variant()
                        )
                        .is_err()
                    );
                    assert!(
                        menus
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|(_, label)| label == "查字")
                    );
                    assert!(
                        menus
                            .lock()
                            .unwrap()
                            .iter()
                            .any(|(_, label)| label == "设置")
                    );
                    call(
                        &client,
                        path,
                        "org.freedesktop.DBus.Properties",
                        "Set",
                        (ENGINE, "ContentType", (8u32, 0u32).to_variant()).to_variant(),
                    )
                    .unwrap();
                    let before = calls.load(Ordering::SeqCst);
                    assert!(!key(u32::from(b'a')), "密码输入必须放行");
                    assert_eq!(calls.load(Ordering::SeqCst), before, "密码按键不能送入引擎");
                    call(&client, path, ENGINE, "FocusOut", ().to_variant()).unwrap();
                    assert!(!key(u32::from(b'a')));
                    // 桌面可能在菜单点击前复位或失焦，但窗口命令不依赖宿主文字快照。
                    for property in ["tigertype-lookup", "tigertype-settings"] {
                        call(
                            &client,
                            path,
                            ENGINE,
                            "PropertyActivate",
                            (property, 0u32).to_variant(),
                        )
                        .unwrap();
                    }
                    assert_eq!(
                        *panels.lock().unwrap(),
                        vec![Panel::Lookup, Panel::Settings]
                    );
                    assert_eq!(
                        calls.load(Ordering::SeqCst),
                        before,
                        "窗口命令不能向引擎发送按键"
                    );
                    call(
                        &client,
                        path,
                        "org.freedesktop.IBus.Service",
                        "Destroy",
                        ().to_variant(),
                    )
                    .unwrap();
                    assert!(call(&client, path, ENGINE, "FocusIn", ().to_variant()).is_err());
                    let mut paths = Vec::new();
                    for _ in 0..16 {
                        let reply = call(
                            &client,
                            "/org/freedesktop/IBus/Factory",
                            FACTORY,
                            "CreateEngine",
                            ("tigertype",).to_variant(),
                        )
                        .unwrap();
                        paths.push(reply.get::<(glib::variant::ObjectPath,)>().unwrap().0);
                    }
                    assert!(
                        call(
                            &client,
                            "/org/freedesktop/IBus/Factory",
                            FACTORY,
                            "CreateEngine",
                            ("tigertype",).to_variant()
                        )
                        .unwrap_err()
                        .to_string()
                        .contains("实例数超限")
                    );
                    for path in paths {
                        call(
                            &client,
                            path.as_str(),
                            "org.freedesktop.IBus.Service",
                            "Destroy",
                            ().to_variant(),
                        )
                        .unwrap();
                    }
                    client.remove_filter(filter);
                });
                main.run();
                worker.join().unwrap();
                assert!(server.engines.borrow().is_empty());
                drop(server);
            })
            .unwrap();
    }
    #[test]
    fn gnome_gjs_decodes_our_wire_payload_and_rejects_invalid_attachments() {
        use gio::prelude::*;
        // Rust 门禁驱动真实 GJS/IBus 解码；脚本仅为适配层执行探针，不接入桌面或用户配置。
        let root = std::env::temp_dir().join(format!(
            "tigertype-gnome-{}-{}",
            std::process::id(),
            glib::monotonic_time()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("gnome");
        let module = gio::File::for_path(source.join("presentation.js")).uri();
        let script = root.join("check.mjs");
        std::fs::write(&script, format!(r#"
import GLib from 'gi://GLib';
import IBus from 'gi://IBus';
import Cairo from 'cairo';
import {{decode, hex, rowColors, shapeRow, paintRow}} from '{module}';
IBus.init();
const require = (ok, message) => {{ if (!ok) throw new Error(message); }};
Reflect.parse(new TextDecoder().decode(GLib.file_get_contents(ARGV[1])[1]), {{target: 'module'}});
const table = IBus.Serializable.deserialize_object(GLib.Variant.parse(null, ARGV[0], null, null));
const model = decode(table);
require(model !== null && model.rows[0].text === '𨑃' && model.rows[0].comment === '〔辰 · fc〕', '实际附件未保留字形/注释');
require(model.colors[7] === Number(ARGV[2]) && hex(15) === '#00000f', '色值不符');
require(rowColors(model.colors, true)[2] === model.colors[7], '高亮文字不符');
const row = shapeRow(model.rows[0], model.font, model.colors, 1);
require(row.width > 0 && row.width <= 900 && row.height > 0, '布局范围错误');
const surface = new Cairo.ImageSurface(Cairo.Format.ARGB32, Math.ceil(row.width), row.height);
const cr = new Cairo.Context(surface);
cr.setSourceRGB(1, 1, 1); cr.paint();
paintRow(cr, row);
surface.writeToPNG(ARGV[3]);
cr.$dispose();
for (const value of [[2, 0, model.colors], [1, 2, model.colors], [1, 0, [0]], [1, 0, [0x1000000, ...model.colors.slice(1)]]]) {{
    table.set_qattachment(GLib.quark_from_string('tigertype-style'), new GLib.Variant('(uuau)', value));
    require(decode(table) === null, '应拒绝非法样式附件');
}}
table.set_qattachment(GLib.quark_from_string('tigertype-style'), new GLib.Variant('(uuau)', [1, 0, model.colors]));
table.get_candidate(0).set_qattachment(GLib.quark_from_string('tigertype-candidate'), new GLib.Variant('(ss)', ['x'.repeat(1025), '']));
require(decode(table) === null, '应拒绝超长文字');
table.set_qattachment(GLib.quark_from_string('tigertype-style'), new GLib.Variant('s', 'invalid'));
require(decode(table) === null, '应拒绝错误协议类型');
print('OK');
"#)).unwrap();
        for theme in tigertype_contracts::Theme::ALL {
            let state = InputState {
                options: tigertype_contracts::Options {
                    theme,
                    ..Default::default()
                },
                candidates: vec![Candidate {
                    text: "𨑃".into(),
                    label: "1".into(),
                    comment: "〔辰 · fc〕".into(),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let output = Command::new("gjs")
                .arg("-m")
                .arg(&script)
                .arg(lookup(&state).print(true).as_str())
                .arg(source.join("extension.js"))
                .arg(state.options.palette().chosen.to_string())
                .arg(root.join(format!("{theme:?}.png")))
                .output()
                .expect("Linux 门禁需要 gjs（GNOME 适配运行时）");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "OK");
        }
    }
    #[test]
    fn serialized_objects_match_ibus_wire_contract() {
        assert_eq!(text("𠀀").type_().as_str(), "(sa{sv}sv)");
        assert_eq!(
            lookup(&InputState::default()).type_().as_str(),
            "(sa{sv}uubbiavav)"
        );
        assert_eq!(
            (text("a"), 1_u32, true, 0_u32)
                .to_variant()
                .type_()
                .as_str(),
            "(vubu)"
        );
        assert!(
            DBusNodeInfo::for_xml(XML)
                .unwrap()
                .lookup_interface(ENGINE)
                .is_some()
        );
        let mut menu = Menu::new("test/engine".into());
        let state = InputState {
            composing: true,
            candidates: vec![Candidate {
                text: "虎".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        menu.update(&state, true, None).unwrap();
        let first = properties(menu.entries())
            .child_value(2)
            .get::<Vec<Variant>>()
            .unwrap();
        menu.update(&state, true, None).unwrap();
        let next = properties(menu.entries())
            .child_value(2)
            .get::<Vec<Variant>>()
            .unwrap();
        for (index, (key, _)) in BASE_PROPERTIES.iter().enumerate() {
            assert_eq!(first[index].child_value(2).str(), Some(*key));
            assert_eq!(first[index].child_value(2), next[index].child_value(2));
        }
        assert_ne!(
            first[5].child_value(2),
            next[5].child_value(2),
            "只有候选动作继续绑定展示版本"
        );
    }
}
