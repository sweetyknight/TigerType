//! Fcitx5 薄插件的有界 D-Bus 服务；只接受框架进程的连接，绝对期限包含排队时间。
use crate::{
    Keyboard,
    menu::{Action, BASE, Menu, caption},
};
use gio::{
    DBusConnection, DBusNodeInfo, RegistrationId,
    glib::{self, Variant, variant::ToVariant},
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    time::{Duration, Instant},
};
use tigertype_contracts::{InputState, Key, KeyOutcome, protocol::Panel};
use tigertype_input_client::{Channel, Host};
pub const NAME: &str = "local.tigertype.Fcitx";
pub const PATH: &str = "/local/tigertype/Fcitx";
pub const INTERFACE: &str = "local.tigertype.Fcitx1";
pub const VERSION: u32 = 1;
const XML: &str = r#"<node><interface name="local.tigertype.Fcitx1"><method name="Event">
<arg type="u" direction="in"/><arg type="t" direction="in"/><arg type="t" direction="in"/><arg type="x" direction="in"/>
<arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/>
<arg type="u" direction="out"/><arg type="t" direction="out"/><arg type="b" direction="out"/><arg type="s" direction="out"/>
<arg type="s" direction="out"/><arg type="a(ss)" direction="out"/><arg type="i" direction="out"/><arg type="s" direction="out"/><arg type="a(ss)" direction="out"/>
</method></interface></node>"#;
struct Context {
    host: Host<Box<dyn Channel>>,
    keyboard: Keyboard,
    sequence: u64,
    menu: Menu,
}
struct State {
    /// 已认证的插件连接。
    owner: String,
    /// Fcitx5 框架名字的当前所有者，空表示尚未查询；由名字变化信号维护，按键路径不逐键查询。
    framework: String,
    contexts: BTreeMap<u64, Context>,
}
pub struct Server {
    bus: DBusConnection,
    registration: Option<RegistrationId>,
    _disconnected: gio::SignalSubscription,
}
fn owner(bus: &DBusConnection, name: &str, timeout: i32) -> Result<String, String> {
    bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "GetNameOwner",
        Some(&(name,).to_variant()),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        timeout,
        gio::Cancellable::NONE,
    )
    .map_err(|e| e.to_string())?
    .get::<(String,)>()
    .map(|value| value.0)
    .ok_or("框架身份回复不符".into())
}
fn remaining(deadline: Instant) -> Result<i32, String> {
    let left = deadline
        .checked_duration_since(Instant::now())
        .ok_or("认证期限已过")?;
    Ok((left.as_micros().div_ceil(1000)).clamp(1, 50) as i32)
}
fn pid(bus: &DBusConnection, name: &str, deadline: Instant) -> Result<u32, String> {
    bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "GetConnectionUnixProcessID",
        Some(&(name,).to_variant()),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        remaining(deadline)?,
        gio::Cancellable::NONE,
    )
    .map_err(|e| e.to_string())?
    .get::<(u32,)>()
    .map(|p| p.0)
    .ok_or("进程身份类型不符".into())
}
impl Server {
    pub fn new(
        bus: DBusConnection,
        channel: impl Fn() -> Box<dyn Channel> + 'static,
        show: impl Fn(Panel) + 'static,
    ) -> Result<Self, String> {
        let reply = bus
            .call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "RequestName",
                Some(&(NAME, 4_u32).to_variant()),
                None,
                gio::DBusCallFlags::NO_AUTO_START,
                2000,
                gio::Cancellable::NONE,
            )
            .map_err(|e| e.to_string())?;
        if reply.get::<(u32,)>() != Some((1,)) {
            return Err("Fcitx 后台名称已被占用".into());
        }
        let identity = bus.unique_name().ok_or("缺少后台总线身份")?.to_string();
        let state = Rc::new(RefCell::new(State {
            owner: String::new(),
            framework: String::new(),
            contexts: BTreeMap::new(),
        }));
        let disconnected = state.clone();
        // 只订阅框架名字的换主：退出/重启时旧连接与全部上下文作废，其他名字变化不唤醒后台。
        let subscription = bus.subscribe_to_signal(
            Some("org.freedesktop.DBus"),
            Some("org.freedesktop.DBus"),
            Some("NameOwnerChanged"),
            Some("/org/freedesktop/DBus"),
            Some("org.fcitx.Fcitx5"),
            gio::DBusSignalFlags::NONE,
            move |signal| {
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    let Some((_, _, new)) = signal.parameters.get::<(String, String, String)>()
                    else {
                        return;
                    };
                    if let Ok(mut state) = disconnected.try_borrow_mut() {
                        state.contexts.clear();
                        state.owner.clear();
                        state.framework = new;
                    }
                }));
            },
        );
        let info = DBusNodeInfo::for_xml(XML)
            .map_err(|e| e.to_string())?
            .lookup_interface(INTERFACE)
            .ok_or("接口缺失")?;
        let registration = bus
            .register_object(PATH, &info)
            .method_call(move |bus, sender, _, _, _, parameters, invocation| {
                let result = catch_unwind(AssertUnwindSafe(|| -> Result<Variant, String> {
                    if parameters.size() > 2048 {
                        return Err("请求过长".into());
                    }
                    let (version, id, sequence, expires, command, symbol, modifiers, token) =
                        parameters
                            .get::<(u32, u64, u64, i64, String, u32, u32, String)>()
                            .ok_or("协议类型不符")?;
                    let left = expires
                        .checked_sub(glib::monotonic_time())
                        .filter(|left| (1..=50_000).contains(left))
                        .ok_or("请求已过期或预算非法")?;
                    if version != VERSION || id == 0 || sequence == 0 {
                        return Err("协议版本或编号不符".into());
                    }
                    let deadline = Instant::now() + Duration::from_micros(left as u64);
                    let sender = sender.ok_or("缺少发送方")?;
                    let mut state = state.borrow_mut();
                    if state.framework.is_empty() {
                        state.framework = owner(&bus, "org.fcitx.Fcitx5", remaining(deadline)?)?;
                    }
                    if state.owner != sender {
                        // Fcitx 主框架与 GIO 胶水使用两条连接；必须由总线核对同一进程，不能只信名字或正文 PID。
                        if sender != state.framework
                            && pid(&bus, sender, deadline)?
                                != pid(&bus, &state.framework, deadline)?
                        {
                            return Err("拒绝非 Fcitx5 进程调用".into());
                        }
                        // 换了连接即作废旧快照，不复用旧会话或上下文编号。
                        state.contexts.clear();
                        state.owner = sender.into();
                    }
                    if command == "base"
                        && let Some(Action::Show(panel)) = base(&token)
                    {
                        // 窗口命令不依赖上下文与焦点：托盘菜单常在焦点回到应用前激活。
                        if let Some(context) = state.contexts.get_mut(&id) {
                            if sequence <= context.sequence {
                                return Err("请求顺序不符".into());
                            }
                            context.sequence = sequence;
                        }
                        show(panel);
                        return Ok(response(
                            sequence,
                            false,
                            "",
                            &InputState::default(),
                            &[],
                            "",
                        ));
                    }
                    if command == "focus" {
                        match state.contexts.get(&id) {
                            Some(context) if sequence <= context.sequence => {
                                return Err("焦点请求重复".into());
                            }
                            // 保留会话：中英、全角与标点状态跨焦点保持，旧组合由下一键的焦点代次清除。
                            Some(_) => {}
                            None if state.contexts.len() >= 16 => {
                                return Err("输入上下文数超限".into());
                            }
                            None => {
                                state.contexts.insert(
                                    id,
                                    Context {
                                        host: Host::new(channel()),
                                        keyboard: Keyboard::default(),
                                        sequence: sequence - 1,
                                        menu: Menu::new(format!(
                                            "{identity}/{sender}/{id}/{sequence}"
                                        )),
                                    },
                                );
                            }
                        }
                    }
                    if command == "close" {
                        if let Some(context) = state.contexts.get(&id)
                            && sequence <= context.sequence
                        {
                            return Err("关闭请求重复".into());
                        }
                        state.contexts.remove(&id);
                        return Ok(response(
                            sequence,
                            false,
                            "",
                            &InputState::default(),
                            &[],
                            "",
                        ));
                    }
                    let context = state.contexts.get_mut(&id).ok_or("上下文尚未激活")?;
                    if sequence <= context.sequence {
                        return Err("请求顺序不符".into());
                    }
                    context.sequence = sequence;
                    let outcome = process(
                        context, &command, symbol, modifiers, &token, deadline, &show,
                    )?;
                    if Instant::now() >= deadline || glib::monotonic_time() >= expires {
                        context.host.close();
                        context.menu.invalidate();
                        return Err("回复已过期，丢弃提交".into());
                    }
                    let caption = if command == "confirm" {
                        "请在输入法菜单确认要上屏的文字".into()
                    } else {
                        caption(&outcome.state)
                    };
                    Ok(response(
                        sequence,
                        outcome.handled,
                        outcome.commit.as_deref().unwrap_or(""),
                        &outcome.state,
                        context.menu.entries(),
                        &caption,
                    ))
                }))
                .unwrap_or_else(|_| {
                    state.borrow_mut().contexts.clear();
                    Err("输入法回调失败".into())
                });
                match result {
                    Ok(reply) => invocation.return_value(Some(&reply)),
                    Err(error) => invocation.return_dbus_error("local.tigertype.Error", &error),
                }
            })
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            bus,
            registration: Some(registration),
            _disconnected: subscription,
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(id) = self.registration.take() {
            let _ = self.bus.unregister_object(id);
        }
        // 单实例名字随连接释放；退出路径不启动框架或等待其他进程。
    }
}
fn response(
    sequence: u64,
    handled: bool,
    commit: &str,
    state: &InputState,
    menu: &[(String, String)],
    caption: &str,
) -> Variant {
    let candidates: Vec<_> = state
        .candidates
        .iter()
        .map(|c| (c.text.as_str(), c.comment.as_str()))
        .collect();
    (
        VERSION,
        sequence,
        handled,
        commit,
        state.composition.text.as_str(),
        candidates,
        state.highlighted.map(|n| n as i32).unwrap_or(-1),
        caption,
        menu,
    )
        .to_variant()
}
fn base(token: &str) -> Option<Action> {
    BASE.iter()
        .find(|(key, _)| *key == token)
        .map(|(_, action)| *action)
}
/// 只保留模式与选项，供复位回复使用；组合文字和候选已撤销。
fn modes(state: &InputState) -> InputState {
    InputState {
        page_size: state.page_size,
        mode: state.mode,
        options: state.options,
        ascii_mode: state.ascii_mode,
        ascii_punctuation: state.ascii_punctuation,
        full_shape: state.full_shape,
        ..Default::default()
    }
}
fn process(
    context: &mut Context,
    command: &str,
    symbol: u32,
    modifiers: u32,
    token: &str,
    deadline: Instant,
    show: &dyn Fn(Panel),
) -> Result<KeyOutcome, String> {
    let empty = |state| KeyOutcome {
        handled: false,
        commit: None,
        state,
    };
    let key = match command {
        "focus" => {
            context.host.blur();
            context.keyboard.reset();
            None
        }
        // 框架复位（含失焦）只撤销组合，保留会话与模式；不像关闭那样丢掉中英等状态。
        "reset" => {
            context.host.discard();
            context.host.blur();
            context.keyboard.reset();
            let state = modes(context.host.state());
            context.menu.update(&state, true, None)?;
            return Ok(empty(state));
        }
        "base" => {
            let Some(Action::Key(key)) = base(token) else {
                return Err("未知基础菜单".into());
            };
            // 固定模式命令只允许空组合（同 IBus），不把切换动作变成原码提交。
            if context.host.state().composing {
                return Err("请先结束组合再切换模式".into());
            }
            Some(key)
        }
        "confirm" => {
            let state = context.host.state();
            if !state.composing || symbol as usize >= state.candidates.len() {
                return Err("候选确认已失效".into());
            }
            context.menu.update(state, true, Some(symbol as usize))?;
            return Ok(empty(state.clone()));
        }
        "menu" => match context.menu.take(token).ok_or("菜单快照已失效")? {
            Action::Show(panel) => {
                show(panel);
                None
            }
            Action::Key(key) => Some(key),
            Action::Select { revision, index } => {
                let outcome = context
                    .host
                    .select(revision, index, deadline)
                    .map_err(|_| "选字通道不可用")?
                    .ok_or("候选快照已失效")?;
                context.menu.update(&outcome.state, true, None)?;
                return Ok(outcome);
            }
        },
        "key" => context
            .keyboard
            .key(symbol, modifiers, context.host.state().composing),
        _ => return Err("未知操作".into()),
    };
    let outcome = if let Some(key) = key {
        if !context.host.ready(deadline) {
            return Err("引擎不可用".into());
        }
        if (key == Key::ToggleLearning && !context.host.state().composing)
            || !context.host.state().consumes(key)
        {
            empty(context.host.state().clone())
        } else {
            context
                .host
                .key(key, deadline)
                .map_err(|_| "按键通道不可用")?
        }
    } else {
        if command == "key"
            && modifiers & (1 << 30) == 0
            && !(0xffe1..=0xffee).contains(&symbol)
            && context.host.state().composing
        {
            context.host.discard();
            context.host.blur();
            context.menu.invalidate();
            return Ok(empty(InputState::default()));
        }
        empty(context.host.state().clone())
    };
    context.menu.update(&outcome.state, true, None)?;
    Ok(outcome)
}
