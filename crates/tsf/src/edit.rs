//! 只在同步写编辑会话内改本服务拥有的组合范围；异步/迟到回调、外部改写与非空用户选区拒绝。
use std::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    time::Instant,
};
use tigertype_contracts::{KeyOutcome, MAX_TEXT_BYTES, candidate::ScreenRect};
use windows::{
    Win32::{
        Foundation::{E_FAIL, E_INVALIDARG},
        UI::TextServices::*,
    },
    core::{Error, HRESULT, Interface, Ref, Result, implement},
};

#[derive(Debug, Default)]
#[must_use]
pub struct EditReport {
    /// SetText 已确认写入提交后不能再重放请求，即使后续 EndComposition/光标更新失败。
    pub committed: bool,
    pub error: Option<HRESULT>,
}
struct State {
    composition: Option<ITfComposition>,
    // 已写入提交后只保留结束权，不再保留可删除/改写提交文本的权限。
    cleanup: Option<ITfComposition>,
    text: Vec<u16>,
    revision: u64,
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
    Update(KeyOutcome),
    Cancel,
    Caret,
}
struct Completion {
    report: EditReport,
    caret: Option<ScreenRect>,
    ran: bool,
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
    deadline: Instant,
    action: Action,
    completion: Rc<RefCell<Completion>>,
}
impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        // 所有系统 COM 回调截住 panic；迟到或重复回调在任何文档访问之前拒绝。
        catch_unwind(AssertUnwindSafe(|| {
            if !self.valid.replace(false)
                || Instant::now() >= self.deadline
                || self.epoch.get() != self.expected_epoch
                || (self.expected_epoch == 0 && !matches!(self.action, Action::Cancel))
            {
                return Err(E_FAIL.into());
            }
            self.completion.borrow_mut().ran = true;
            self.execute(cookie)
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
    fn execute(&self, cookie: u32) -> Result<()> {
        if matches!(self.action, Action::Caret) {
            let selection = selected(&self.context, cookie)?;
            let mut rect = windows::Win32::Foundation::RECT::default();
            let mut clipped = false.into();
            // SAFETY: 只在读编辑会话内取得当前真实布局，布局不可用不猜光标位置。
            unsafe {
                self.context.GetActiveView()?.GetTextExt(
                    cookie,
                    &selection,
                    &mut rect,
                    &mut clipped,
                )?;
            }
            let caret = ScreenRect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            };
            if !caret.valid() || clipped.as_bool() {
                return Err(E_FAIL.into());
            }
            self.completion.borrow_mut().caret = Some(caret);
            return Ok(());
        }
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
        if self.terminated.get() {
            return Err(E_FAIL.into());
        }
        // SAFETY: 在同一编辑会话内查询宿主状态，只读上下文不尝试写入。
        if unsafe { self.context.GetStatus()? }.dwDynamicFlags & TS_SD_READONLY != 0 {
            return Err(E_FAIL.into());
        }
        // 取出拥有权再调用 COM，避免持 RefCell 借用跨越宿主的同步重入。
        let (composition, text, revision) = {
            let state = self.state.try_borrow().map_err(|_| Error::from(E_FAIL))?;
            (
                state.composition.clone(),
                state.text.clone(),
                state.revision,
            )
        };
        if let Action::Update(outcome) = &self.action
            && outcome.state.revision <= revision
        {
            return Err(E_FAIL.into());
        }
        let range = if let Some(composition) = &composition {
            // SAFETY: composition 引用由该上下文的 StartComposition 返回并持有。
            let range = unsafe { composition.GetRange()? };
            if !matches_text(&range, cookie, &text)? {
                return Err(E_FAIL.into());
            }
            range
        } else {
            if matches!(self.action, Action::Cancel) {
                return Ok(());
            }
            let range = selected(&self.context, cookie)?;
            // SAFETY: 新组合不覆盖用户的非空选区，拒绝后由宿主处理原键。
            if !unsafe { range.IsEmpty(cookie)? }.as_bool() {
                return Err(E_FAIL.into());
            }
            range
        };
        if matches!(self.action, Action::Cancel) {
            // SAFETY: 已核对范围仍等于本服务写入的组合，绝不删除宿主改过的正文。
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
            return Ok(());
        }
        let Action::Update(outcome) = &self.action else {
            return Err(E_FAIL.into());
        };
        if composition.is_some() {
            let selection = selected(&self.context, cookie)?;
            // SAFETY: 若用户移动了光标或选区，不得把后来收到的结果写回旧编辑位置。
            unsafe {
                if !selection.IsEmpty(cookie)?.as_bool()
                    || range.CompareEnd(cookie, &selection, TF_ANCHOR_END)? != 0
                {
                    return Err(E_FAIL.into());
                }
            }
        }
        if Instant::now() >= self.deadline
            || self.terminated.get()
            || self.epoch.get() != self.expected_epoch
        {
            return Err(E_FAIL.into());
        }
        let mut range = range;
        let mut active = composition;
        // 首次写入尝试前消耗快照，后续 COM 清理失败也不能重放这次提交。
        self.state.borrow_mut().revision = outcome.state.revision;
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
            if Instant::now() >= self.deadline
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
            if Instant::now() >= self.deadline
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
            if !outcome.state.composing {
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
        self.state.borrow_mut().revision = outcome.state.revision;
        Ok(())
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
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
                revision: 0,
            })),
            terminated: Rc::new(Cell::new(false)),
            composition_generation: Rc::new(Cell::new(0)),
            busy: Cell::new(false),
        })
    }
    /// 失焦先使旧回调代次失效；取消只可另在原上下文的合法编辑会话中执行。
    pub fn invalidate(&self) {
        self.epoch.set(0);
    }
    /// 错误不代表未写入：调用方必须检查 committed，不重放已确认提交，并取消剩余自有组合。
    pub fn apply(&self, outcome: KeyOutcome, deadline: Instant) -> EditReport {
        if outcome.state.composition.text.len() > MAX_TEXT_BYTES
            || outcome
                .commit
                .as_ref()
                .is_some_and(|s| s.len() > MAX_TEXT_BYTES)
        {
            return EditReport {
                committed: false,
                error: Some(E_INVALIDARG),
            };
        }
        self.run(Action::Update(outcome), deadline).report
    }
    pub fn cancel(&self, deadline: Instant) -> EditReport {
        self.run(Action::Cancel, deadline).report
    }
    pub fn caret(&self, deadline: Instant) -> Result<ScreenRect> {
        let completed = self.run(Action::Caret, deadline);
        completed
            .caret
            .ok_or_else(|| completed.report.error.unwrap_or(E_FAIL).into())
    }
    fn run(&self, action: Action, deadline: Instant) -> Completion {
        if self.busy.replace(true) {
            return Completion {
                report: EditReport {
                    committed: false,
                    error: Some(E_FAIL),
                },
                caret: None,
                ran: false,
            };
        }
        let completion = Rc::new(RefCell::new(Completion {
            report: EditReport::default(),
            caret: None,
            ran: false,
        }));
        let valid = Rc::new(Cell::new(true));
        let read = matches!(action, Action::Caret);
        let session: ITfEditSession = EditSession {
            context: self.context.clone(),
            state: self.state.clone(),
            epoch: self.epoch.clone(),
            expected_epoch: self.epoch.get(),
            terminated: self.terminated.clone(),
            composition_generation: self.composition_generation.clone(),
            valid: valid.clone(),
            deadline,
            action,
            completion: completion.clone(),
        }
        .into();
        // SAFETY: 只请求同步会话，不允许调度迟到提交；返回后 valid 立即撤销，即使宿主违反同步约定也不能再写。
        let result = unsafe {
            self.context.RequestEditSession(
                self.client,
                &session,
                TF_ES_SYNC | if read { TF_ES_READ } else { TF_ES_READWRITE },
            )
        }
        .and_then(|hr| hr.ok());
        valid.set(false);
        self.busy.set(false);
        let mut completed = completion.borrow_mut();
        if let Err(error) = result {
            completed.report.error = Some(error.code());
        }
        if !completed.ran {
            completed.report.error = Some(E_FAIL);
        }
        Completion {
            report: std::mem::take(&mut completed.report),
            caret: completed.caret,
            ran: completed.ran,
        }
    }
}
