//! 只在编辑会话内改本服务拥有的组合范围。按键/点击会话先核对文档，再调用后台交换，最后写入并取组合位置，
//! 均在同一会话内完成：拿不到锁时后台状态未变；迟到回调、外部改写与非空用户选区不写入。
use std::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    time::{Duration, Instant},
};
use tigertype_contracts::{KeyOutcome, MAX_TEXT_BYTES};
use windows::{
    Win32::{
        Foundation::{E_FAIL, E_INVALIDARG, HWND, RECT},
        UI::TextServices::*,
    },
    core::{Error, HRESULT, IUnknown, Interface, Ref, Result, implement},
};

/// 失焦取消只处理本地文档，不等待后台；排队的取消与点击超过此期限才开始即作废。
const CANCEL_BUDGET: Duration = Duration::from_millis(50);
const QUEUED: Duration = Duration::from_secs(2);
/// 点击会话的后台预算从会话开始计算，与按键回调同为 50ms。
const SELECT_BUDGET: Duration = Duration::from_millis(50);

/// 后台交换的结论；决定会话是写入、原样放弃，还是撤回自有组合。
// 每个会话只移动一次，不值得为缩小枚举而分配。
#[allow(clippy::large_enum_variant)]
pub enum Verdict {
    /// 未发出请求或后台明确未处理（旧快照、焦点被夺等），状态未变：不写入、不作废。
    Skip,
    /// 结果不确定：撤回仍属本服务的组合，原键放行，不重放。
    Lost,
    Apply(KeyOutcome),
}
/// 参数一为 true 表示文档里的旧组合已被宿主终止或改写，后台须先清空；参数二是本会话的绝对期限。
type Exchange = Box<dyn FnOnce(bool, Instant) -> Verdict>;
type Finish = Box<dyn FnOnce(EditReport)>;

#[derive(Debug, Default)]
#[must_use]
pub struct EditReport {
    /// 已有后台结论（写入或不确定）：后台状态可能已变，之后失败须作废且不重放。
    pub exchanged: bool,
    /// 引擎处理了该键且结果已完整写入；只有此时（或已提交）才吃掉原键。
    pub handled: bool,
    /// SetText 已确认写入提交后不能再重放请求，即使后续 EndComposition/光标更新失败。
    pub committed: bool,
    pub error: Option<HRESULT>,
    /// 写入后组合的屏幕矩形；布局不可用时为 None，由候选窗另行回退。
    pub caret: Option<RECT>,
    /// 上下文视图窗口，作候选窗 owner；取不到时为空。
    pub owner: HWND,
}
struct State {
    composition: Option<ITfComposition>,
    // 已写入提交后只保留结束权，不再保留可删除/改写提交文本的权限。
    cleanup: Option<ITfComposition>,
    text: Vec<u16>,
}
/// Rc 限制在宿主 STA 使用；每个上下文独立拥有组合，不用全局表或裸指针共享业务状态。
pub struct Editor {
    context: ITfContext,
    client: u32,
    epoch: Rc<Cell<u64>>,
    state: Rc<RefCell<State>>,
    terminated: Rc<Cell<bool>>,
    composition_generation: Rc<Cell<u64>>,
    busy: Cell<bool>,
}
enum Action {
    Key(RefCell<Option<Exchange>>),
    /// 点击只针对仍完整存在的自有组合，不结束旧组合、不请求清空。
    Select(RefCell<Option<Exchange>>),
    Cancel,
}
struct Completion {
    report: EditReport,
    ran: bool,
}
struct Request {
    queued: bool,
    ran: bool,
    report: EditReport,
}

#[implement(ITfCompositionSink)]
struct CompositionSink {
    terminated: Rc<Cell<bool>>,
    generation: Rc<Cell<u64>>,
    expected: u64,
}
impl ITfCompositionSink_Impl for CompositionSink_Impl {
    fn OnCompositionTerminated(&self, _: u32, _: Ref<ITfComposition>) -> Result<()> {
        // 此回调可能重入 SetText/EndComposition；只记原子于 STA 的标志，不借用编辑状态。
        if self.generation.get() == self.expected {
            self.terminated.set(true);
        }
        Ok(())
    }
}
#[implement(ITfEditSession)]
struct EditSession {
    context: ITfContext,
    state: Rc<RefCell<State>>,
    epoch: Rc<Cell<u64>>,
    expected_epoch: u64,
    terminated: Rc<Cell<bool>>,
    composition_generation: Rc<Cell<u64>>,
    valid: Rc<Cell<bool>>,
    deadline: Cell<Instant>,
    /// 排队会话在开始时才计算后台预算。
    budget: Option<Duration>,
    action: Action,
    completion: Rc<RefCell<Completion>>,
    finish: RefCell<Option<Finish>>,
}
impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        // 所有系统 COM 回调截住 panic；迟到或重复回调在任何文档访问之前拒绝。
        catch_unwind(AssertUnwindSafe(|| {
            if !self.valid.replace(false)
                || Instant::now() >= self.deadline.get()
                || self.epoch.get() != self.expected_epoch
                || (self.expected_epoch == 0 && !matches!(self.action, Action::Cancel))
            {
                return Err(E_FAIL.into());
            }
            if let Some(budget) = self.budget {
                self.deadline.set(Instant::now() + budget);
            }
            self.completion.borrow_mut().ran = true;
            let result = self.execute(cookie);
            if result.is_err() && self.completion.borrow().report.exchanged {
                // 后台已有结论而写入失败：同一会话撤回仍一致的自有组合，已提交文字不动。
                let _ = self.withdraw(cookie);
            }
            if let Err(error) = &result {
                self.completion.borrow_mut().report.error = Some(error.code());
            }
            if let Some(finish) = self.finish.take() {
                finish(std::mem::take(&mut self.completion.borrow_mut().report));
            }
            result
        }))
        .unwrap_or_else(|_| Err(E_FAIL.into()))
    }
}

fn selected(context: &ITfContext, cookie: u32) -> Result<ITfRange> {
    let mut selection = TF_SELECTION::default();
    let mut fetched = 0;
    // SAFETY: 在系统发放的编辑 cookie 内，输出结构初始化为 None；无论 HRESULT 如何都接管/释放返回引用。
    let result = unsafe {
        context.GetSelection(
            cookie,
            TF_DEFAULT_SELECTION,
            std::slice::from_mut(&mut selection),
            &mut fetched,
        )
    };
    // SAFETY: 从 ManuallyDrop 取出唯一返回引用，随后由 Option 正常回收。
    let range = unsafe { ManuallyDrop::take(&mut selection.range) };
    result?;
    if fetched != 1 {
        return Err(E_FAIL.into());
    }
    range.ok_or_else(|| E_FAIL.into())
}
fn matches_text(range: &ITfRange, cookie: u32, expected: &[u16]) -> Result<bool> {
    let mut text = vec![0u16; expected.len() + 1];
    let mut count = 0;
    // SAFETY: 只读取本服务记录的有界组合范围；多读一个单元检测宿主外部扩展。
    unsafe {
        range.GetText(cookie, 0, &mut text, &mut count)?;
    }
    Ok(count as usize == expected.len() && &text[..expected.len()] == expected)
}
/// 组合文本仍是本服务写入的内容；按键还要求光标是组合末端的空插入点。
fn intact(
    context: &ITfContext,
    range: &ITfRange,
    cookie: u32,
    text: &[u16],
    key: bool,
) -> Result<bool> {
    if !matches_text(range, cookie, text)? {
        return Ok(false);
    }
    if !key {
        return Ok(true);
    }
    let selection = selected(context, cookie)?;
    // SAFETY: 同一编辑 cookie 内比较本服务组合与当前选区，不修改任何范围。
    unsafe {
        Ok(selection.IsEmpty(cookie)?.as_bool()
            && range.CompareEnd(cookie, &selection, TF_ANCHOR_END)? == 0)
    }
}
fn set_caret(context: &ITfContext, range: &ITfRange, cookie: u32) -> Result<()> {
    // SAFETY: Clone 返回独立引用，只折叠克隆范围，不能把真实组合范围折叠为空。
    let caret = unsafe { range.Clone()? };
    // SAFETY: 同一个有效编辑 cookie，选择只在刚编辑的范围末端。
    unsafe {
        caret.Collapse(cookie, TF_ANCHOR_END)?;
    }
    let mut selection = TF_SELECTION {
        range: ManuallyDrop::new(Some(caret)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_END,
            fInterimChar: false.into(),
        },
    };
    // SAFETY: SetSelection 借用数组，调用返回后释放本地克隆引用。
    let result = unsafe { context.SetSelection(cookie, std::slice::from_ref(&selection)) };
    // SAFETY: 显式释放 ManuallyDrop 中尚未取走的唯一引用。
    unsafe {
        ManuallyDrop::drop(&mut selection.range);
    }
    result
}
impl EditSession_Impl {
    /// 丢弃本地组合记录并作废旧组合 sink；正文已归宿主，不在这里改动。
    fn forget(&self) -> Result<()> {
        let mut state = self
            .state
            .try_borrow_mut()
            .map_err(|_| Error::from(E_FAIL))?;
        state.composition = None;
        state.text.clear();
        drop(state);
        let generation = self
            .composition_generation
            .get()
            .checked_add(1)
            .ok_or(E_FAIL)?;
        self.composition_generation.set(generation);
        self.terminated.set(false);
        Ok(())
    }
    /// 删除已核对仍等于本服务写入内容的组合，绝不删除宿主改过的正文。
    fn discard(&self, range: &ITfRange, cookie: u32) -> Result<()> {
        // SAFETY: 调用方已在同一 cookie 内核对 range 是自有组合且文本未被改写。
        unsafe {
            range.SetText(cookie, 0, &[])?;
        }
        self.state.borrow_mut().text.clear();
        let composition = self.state.borrow_mut().composition.take();
        self.state.borrow_mut().cleanup = composition.clone();
        if let Some(composition) = composition {
            // SAFETY: 结束的是本服务创建的组合，没有改已确认提交。
            unsafe {
                composition.EndComposition(cookie)?;
            }
        }
        self.state.borrow_mut().cleanup = None;
        self.terminated.set(false);
        Ok(())
    }
    /// 撤回仍与记录一致的自有组合；已被改写则不动正文。
    fn withdraw(&self, cookie: u32) -> Result<()> {
        let (composition, text) = {
            let state = self.state.try_borrow().map_err(|_| Error::from(E_FAIL))?;
            (state.composition.clone(), state.text.clone())
        };
        let Some(composition) = composition else {
            return Ok(());
        };
        // SAFETY: 组合引用由本上下文创建并持有。
        let range = unsafe { composition.GetRange()? };
        if matches_text(&range, cookie, &text)? {
            self.discard(&range, cookie)?;
        }
        Ok(())
    }
    /// 组合位置与视图窗口只用于摆放候选窗；取不到不影响已完成的写入。
    fn locate(&self, range: &ITfRange, cookie: u32) {
        // SAFETY: 同一编辑会话内读取当前视图与布局，失败只是缺少位置。
        let Ok(view) = (unsafe { self.context.GetActiveView() }) else {
            return;
        };
        let mut report = self.completion.borrow_mut();
        // SAFETY: 只读视图属性。
        report.report.owner = unsafe { view.GetWnd() }.unwrap_or_default();
        let mut rect = RECT::default();
        let mut clipped = false.into();
        // SAFETY: range 是本会话刚写入的自有组合，cookie 有效。
        if unsafe { view.GetTextExt(cookie, range, &mut rect, &mut clipped) }.is_ok() {
            report.report.caret = Some(rect);
        }
    }
    fn execute(&self, cookie: u32) -> Result<()> {
        let cleanup = self
            .state
            .try_borrow()
            .map_err(|_| Error::from(E_FAIL))?
            .cleanup
            .clone();
        if let Some(cleanup) = cleanup {
            // SAFETY: 重试的只有 EndComposition，不触碰先前已经确认写入的提交文本。
            unsafe {
                cleanup.EndComposition(cookie)?;
            }
            self.state.borrow_mut().cleanup = None;
            self.terminated.set(false);
        }
        let key = matches!(self.action, Action::Key(_));
        let select = matches!(self.action, Action::Select(_));
        let mut reset = self.terminated.get();
        if reset {
            // 点击不处理终止：保留标记，由下一按键先清空后台。
            if select {
                return Err(E_FAIL.into());
            }
            // 宿主终止旧组合后正文归宿主：只丢本地记录，按键前让后台先清空。
            self.forget()?;
        }
        // SAFETY: 在同一编辑会话内查询宿主状态，只读上下文不尝试写入。
        if unsafe { self.context.GetStatus()? }.dwDynamicFlags & TS_SD_READONLY != 0 {
            return Err(E_FAIL.into());
        }
        // 取出拥有权再调用 COM，避免持 RefCell 借用跨越宿主的同步重入。
        let (mut composition, text) = {
            let state = self.state.try_borrow().map_err(|_| Error::from(E_FAIL))?;
            (state.composition.clone(), state.text.clone())
        };
        let mut owned = None;
        if let Some(current) = &composition {
            // SAFETY: composition 引用由该上下文的 StartComposition 返回并持有。
            let range = unsafe { current.GetRange()? };
            if intact(&self.context, &range, cookie, &text, key)? {
                owned = Some(range);
            } else if key {
                // 外部改写或光标移出：只结束旧组合、不改正文，后台先清空后按当前插入点处理本键。
                // SAFETY: 结束本服务创建的组合不改变文本；可能同步重入终止通知，随后 forget 作废它。
                unsafe {
                    current.EndComposition(cookie)?;
                }
                self.forget()?;
                composition = None;
                reset = true;
            } else {
                return Err(E_FAIL.into());
            }
        }
        let exchange = match &self.action {
            Action::Cancel => {
                return match owned {
                    Some(range) => self.discard(&range, cookie),
                    None => Ok(()),
                };
            }
            Action::Key(exchange) | Action::Select(exchange) => exchange,
        };
        let range = match owned {
            Some(range) => range,
            None if select => return Err(E_FAIL.into()),
            None => {
                let range = selected(&self.context, cookie)?;
                // SAFETY: 新组合不覆盖用户的非空选区，拒绝后由宿主处理原键。
                if !unsafe { range.IsEmpty(cookie)? }.as_bool() {
                    return Err(E_FAIL.into());
                }
                range
            }
        };
        let exchange = exchange.borrow_mut().take().ok_or(E_FAIL)?;
        let outcome = match exchange(reset, self.deadline.get()) {
            Verdict::Skip => {
                // 后台未收到本键：旧组合若已在本会话结束，下次仍须先清空后台。
                self.terminated.set(reset);
                return Err(E_FAIL.into());
            }
            Verdict::Lost => {
                self.completion.borrow_mut().report.exchanged = true;
                return Err(E_FAIL.into());
            }
            Verdict::Apply(outcome) => {
                self.completion.borrow_mut().report.exchanged = true;
                outcome
            }
        };
        if outcome.state.composition.text.len() > MAX_TEXT_BYTES
            || outcome
                .commit
                .as_ref()
                .is_some_and(|s| s.len() > MAX_TEXT_BYTES)
        {
            return Err(E_INVALIDARG.into());
        }
        if !outcome.handled {
            return Ok(());
        }
        if Instant::now() >= self.deadline.get()
            || self.terminated.get()
            || self.epoch.get() != self.expected_epoch
        {
            return Err(E_FAIL.into());
        }
        let mut range = range;
        let mut active = composition;
        if let Some(commit) = &outcome.commit {
            let text: Vec<_> = commit.encode_utf16().collect();
            // SAFETY: 仅在写编辑会话内替换已核对的自有组合/空插入点。
            unsafe {
                range.SetText(cookie, 0, &text)?;
            }
            self.completion.borrow_mut().report.committed = true;
            // 已确认写入的提交不再属于可取消的 preedit，即使随后清理失败也不可删除或重放。
            {
                let mut state = self.state.borrow_mut();
                state.composition = None;
                state.text.clear();
                state.cleanup = active.clone();
            }
            if let Some(old) = active.take() {
                // SAFETY: 此引用属于旧的自有组合；结束时可能重入终止通知。
                unsafe {
                    old.EndComposition(cookie)?;
                }
                self.state.borrow_mut().cleanup = None;
            }
            self.terminated.set(false);
            // SAFETY: 后继组合只从已提交范围末端开始，绝不重新覆盖提交前缀。
            unsafe {
                range = range.Clone()?;
                range.Collapse(cookie, TF_ANCHOR_END)?;
            }
        }
        let next: Vec<_> = outcome.state.composition.text.encode_utf16().collect();
        if outcome.state.composing && active.is_none() {
            if Instant::now() >= self.deadline.get()
                || self.terminated.get()
                || self.epoch.get() != self.expected_epoch
            {
                return Err(E_FAIL.into());
            }
            let generation = self
                .composition_generation
                .get()
                .checked_add(1)
                .ok_or(E_FAIL)?;
            self.composition_generation.set(generation);
            let sink: ITfCompositionSink = CompositionSink {
                terminated: self.terminated.clone(),
                generation: self.composition_generation.clone(),
                expected: generation,
            }
            .into();
            let owner: ITfContextComposition = self.context.cast()?;
            // SAFETY: 空插入点和当前 cookie 同属绑定上下文，sink 通过 COM 引用计数持有。
            active = Some(unsafe { owner.StartComposition(cookie, &range, &sink)? });
            self.state.borrow_mut().composition = active.clone();
        }
        if let Some(active) = active {
            if Instant::now() >= self.deadline.get()
                || self.terminated.get()
                || self.epoch.get() != self.expected_epoch
            {
                return Err(E_FAIL.into());
            }
            // SAFETY: range 是本服务的组合，不把 UTF-8 字节位置直接用于 UTF-16 范围。
            unsafe {
                range.SetText(cookie, 0, &next)?;
            }
            self.state.borrow_mut().text = next;
            set_caret(&self.context, &range, cookie)?;
            if outcome.state.composing {
                self.locate(&range, cookie);
            } else {
                {
                    let mut state = self.state.borrow_mut();
                    state.composition = None;
                    state.cleanup = Some(active.clone());
                }
                // SAFETY: 当前持有的组合引用有效，结束后不再作为可取消内容。
                unsafe {
                    active.EndComposition(cookie)?;
                }
                self.state.borrow_mut().cleanup = None;
                self.terminated.set(false);
            }
        } else if outcome.commit.is_some() {
            set_caret(&self.context, &range, cookie)?;
        }
        self.completion.borrow_mut().report.handled = true;
        Ok(())
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
pub(crate) mod tests;
impl Editor {
    pub fn new(context: ITfContext, client: u32, epoch: u64) -> Result<Self> {
        if client == 0 || epoch == 0 {
            return Err(E_INVALIDARG.into());
        }
        Ok(Self {
            context,
            client,
            epoch: Rc::new(Cell::new(epoch)),
            state: Rc::new(RefCell::new(State {
                composition: None,
                cleanup: None,
                text: Vec::new(),
            })),
            terminated: Rc::new(Cell::new(false)),
            composition_generation: Rc::new(Cell::new(0)),
            busy: Cell::new(false),
        })
    }
    /// 以 IUnknown 比较 COM 身份，不依赖宿主每次返回同一个接口指针。
    pub fn is(&self, context: &ITfContext) -> bool {
        matches!(
            (self.context.cast::<IUnknown>(), context.cast::<IUnknown>()),
            (Ok(a), Ok(b)) if a == b
        )
    }
    /// 本地仍持有未被宿主终止的组合；供按键预判，不访问文档。
    pub fn composing(&self) -> bool {
        !self.terminated.get()
            && self
                .state
                .try_borrow()
                .is_ok_and(|state| state.composition.is_some())
    }
    /// 失焦先使旧回调代次失效；取消只可另在原上下文的合法编辑会话中执行。
    pub fn invalidate(&self) {
        self.epoch.set(0);
    }
    /// 按键会话（同步）：文档核对通过后才调用 exchange 取得后台结论并写入。
    /// 错误不代表未写入：调用方须检查 exchanged/committed，不重放。
    pub fn key(
        &self,
        deadline: Instant,
        exchange: impl FnOnce(bool, Instant) -> Verdict + 'static,
    ) -> EditReport {
        let action = Action::Key(RefCell::new(Some(Box::new(exchange))));
        self.run(action, deadline, true, None, None).report
    }
    /// 候选点击会话（`TF_ES_ASYNCDONTCARE`）：开始时核对组合仍完整，后台预算从开始时计算；
    /// finish 在会话内收到报告，会话始终未执行则不会调用。返回请求是否被宿主受理。
    pub fn select(
        &self,
        exchange: impl FnOnce(Instant) -> Verdict + 'static,
        finish: impl FnOnce(EditReport) + 'static,
    ) -> bool {
        let action = Action::Select(RefCell::new(Some(Box::new(move |_, deadline| {
            exchange(deadline)
        }))));
        self.run(
            action,
            Instant::now() + QUEUED,
            false,
            Some(SELECT_BUDGET),
            Some(Box::new(finish)),
        )
        .queued
    }
    pub fn cancel(&self, deadline: Instant) -> EditReport {
        self.run(Action::Cancel, deadline, true, None, None).report
    }
    /// 失焦/停用：先作废旧回调，再同步取消自有组合；拿不到同步锁时排队一次异步取消，
    /// 它同样只删除仍一致的自有组合，不调用后台。
    pub fn abandon(&self) {
        self.invalidate();
        if !self
            .run(
                Action::Cancel,
                Instant::now() + CANCEL_BUDGET,
                true,
                None,
                None,
            )
            .ran
        {
            let _ = self.run(Action::Cancel, Instant::now() + QUEUED, false, None, None);
        }
    }
    fn run(
        &self,
        action: Action,
        deadline: Instant,
        sync: bool,
        budget: Option<Duration>,
        finish: Option<Finish>,
    ) -> Request {
        if self.busy.replace(true) {
            return Request {
                queued: false,
                ran: false,
                report: EditReport {
                    error: Some(E_FAIL),
                    ..Default::default()
                },
            };
        }
        let completion = Rc::new(RefCell::new(Completion {
            report: EditReport::default(),
            ran: false,
        }));
        let valid = Rc::new(Cell::new(true));
        // 按键必须同步完成；只有取消与点击可以排队，后者开始时重新核对全部条件。
        let sync = sync || matches!(action, Action::Key(_));
        let session: ITfEditSession = EditSession {
            context: self.context.clone(),
            state: self.state.clone(),
            epoch: self.epoch.clone(),
            expected_epoch: self.epoch.get(),
            terminated: self.terminated.clone(),
            composition_generation: self.composition_generation.clone(),
            valid: valid.clone(),
            deadline: Cell::new(deadline),
            budget,
            action,
            completion: completion.clone(),
            finish: RefCell::new(finish),
        }
        .into();
        let flags = TF_ES_READWRITE
            | if sync {
                TF_ES_SYNC
            } else {
                TF_ES_ASYNCDONTCARE
            };
        // SAFETY: 同步会话返回后 valid 立即撤销，即使宿主违反同步约定也不能再写；
        // 排队会话保持有效至首次执行，开始时重新核对代次、期限与组合。
        let result = unsafe {
            self.context
                .RequestEditSession(self.client, &session, flags)
        };
        if sync {
            valid.set(false);
        }
        self.busy.set(false);
        let mut completed = completion.borrow_mut();
        match &result {
            Ok(hr) if hr.is_err() => completed.report.error = Some(*hr),
            Err(error) => completed.report.error = Some(error.code()),
            Ok(_) => {}
        }
        if !completed.ran {
            completed.report.error = Some(E_FAIL);
        }
        Request {
            queued: result.is_ok(),
            ran: completed.ran,
            report: std::mem::take(&mut completed.report),
        }
    }
}
