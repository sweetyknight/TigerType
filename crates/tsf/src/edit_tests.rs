//! 真正 COM 接口/vtable 的合成文本宿主；不激活系统输入法、不碰剪贴板或用户窗口。
use super::*;
use std::time::Duration;
use windows::{
    Win32::{
        Foundation::{E_NOTIMPL, POINT},
        System::Com::IDataObject,
    },
    core::{BOOL, GUID, IUnknown, PCWSTR, PWSTR},
};

#[derive(Default)]
pub(crate) struct Document {
    pub(crate) text: RefCell<Vec<u16>>,
    pub(crate) selection: Cell<(usize, usize)>,
    pub(crate) sink: RefCell<Option<ITfCompositionSink>>,
    readonly: Cell<bool>,
    fail_end: Cell<bool>,
    fail_write: Cell<bool>,
    writes: Cell<u32>,
    ends: Cell<u32>,
    pub(crate) defer: Cell<bool>,
    pub(crate) deferred: RefCell<Option<ITfEditSession>>,
    /// 模拟宿主尚未完成布局：GetTextExt 失败。
    no_layout: Cell<bool>,
}
#[implement(ITfContext, ITfContextComposition)]
struct Context {
    doc: Rc<Document>,
}
/// 合成视图：每个 UTF-16 单元宽 10 像素，从屏幕 (100, 100) 起排一行；视图窗口为空。
#[implement(ITfContextView)]
struct View {
    doc: Rc<Document>,
}
impl ITfContextView_Impl for View_Impl {
    fn GetRangeFromPoint(&self, _: u32, _: *const POINT, _: u32) -> Result<ITfRange> {
        Err(E_NOTIMPL.into())
    }
    fn GetTextExt(
        &self,
        cookie: u32,
        target: Ref<ITfRange>,
        rect: *mut RECT,
        clipped: *mut BOOL,
    ) -> Result<()> {
        if self.doc.no_layout.get() {
            return Err(TS_E_NOLAYOUT.into());
        }
        let target = target.as_ref().ok_or(E_INVALIDARG)?;
        let origin = range(&self.doc, 0, 0);
        // SAFETY: 范围属于本合成文档，偏移从同一 cookie 读取；输出指针由调用方提供。
        unsafe {
            let start = target.CompareStart(cookie, &origin, TF_ANCHOR_START)?;
            let end = target.CompareEnd(cookie, &origin, TF_ANCHOR_START)?;
            rect.write(RECT {
                left: 100 + start * 10,
                top: 100,
                right: 100 + end * 10,
                bottom: 120,
            });
            clipped.write(false.into());
        }
        Ok(())
    }
    fn GetScreenExt(&self) -> Result<RECT> {
        Err(E_NOTIMPL.into())
    }
    fn GetWnd(&self) -> Result<HWND> {
        Ok(HWND::default())
    }
}
#[implement(ITfRange)]
struct Range {
    doc: Rc<Document>,
    start: Cell<usize>,
    end: Cell<usize>,
}
#[implement(ITfComposition)]
struct Composition {
    doc: Rc<Document>,
    range: ITfRange,
}
fn range(doc: &Rc<Document>, start: usize, end: usize) -> ITfRange {
    Range {
        doc: doc.clone(),
        start: Cell::new(start),
        end: Cell::new(end),
    }
    .into()
}
fn outcome(revision: u64, preedit: &str, commit: Option<&str>) -> KeyOutcome {
    KeyOutcome {
        handled: true,
        commit: commit.map(str::to_owned),
        state: tigertype_contracts::InputState {
            revision,
            composing: !preedit.is_empty(),
            composition: tigertype_contracts::Composition {
                text: preedit.into(),
                cursor: preedit.len(),
                selection: 0..preedit.len(),
            },
            ..Default::default()
        },
    }
}
pub(crate) fn document() -> (Rc<Document>, ITfContext) {
    let doc = Rc::new(Document::default());
    let context: ITfContext = Context { doc: doc.clone() }.into();
    (doc, context)
}
fn setup() -> (Rc<Document>, Editor) {
    let (doc, context) = document();
    (doc, Editor::new(context, 1, 1, || {}).unwrap())
}
fn apply(editor: &Editor, outcome: KeyOutcome, deadline: Instant) -> EditReport {
    editor.key(deadline, move |_, _| Verdict::Apply(outcome))
}
pub(crate) fn text(doc: &Document) -> String {
    String::from_utf16(&doc.text.borrow()).unwrap()
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(2)
}

impl ITfContext_Impl for Context_Impl {
    fn RequestEditSession(
        &self,
        _: u32,
        edit: Ref<ITfEditSession>,
        _: TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    ) -> Result<HRESULT> {
        if self.doc.defer.get() {
            *self.doc.deferred.borrow_mut() = edit.as_ref().cloned();
            return Ok(HRESULT(0));
        }
        // SAFETY: 合成宿主在当前线程同步发放固定 cookie，不保存或迟延编辑对象。
        Ok(
            unsafe { edit.as_ref().ok_or(E_INVALIDARG)?.DoEditSession(1) }
                .map_or_else(|e| e.code(), |_| HRESULT(0)),
        )
    }
    fn InWriteSession(&self, _: u32) -> Result<BOOL> {
        Ok(true.into())
    }
    fn GetSelection(
        &self,
        _: u32,
        _: u32,
        count: u32,
        selection: *mut TF_SELECTION,
        fetched: *mut u32,
    ) -> Result<()> {
        if count != 1 || selection.is_null() || fetched.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let (start, end) = self.doc.selection.get();
        // SAFETY: COM 调用者提供一个输出结构和数量指针；返回一个新的拥有权引用。
        unsafe {
            selection.write(TF_SELECTION {
                range: ManuallyDrop::new(Some(range(&self.doc, start, end))),
                style: TF_SELECTIONSTYLE::default(),
            });
            fetched.write(1);
        }
        Ok(())
    }
    fn SetSelection(&self, cookie: u32, count: u32, selection: *const TF_SELECTION) -> Result<()> {
        if count != 1 || selection.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: 调用者借用一个有效选择结构；通过范围比较接口取得测试宿主内偏移。
        let selected = unsafe { &*selection }.range.as_ref().ok_or(E_INVALIDARG)?;
        let origin = range(&self.doc, 0, 0);
        // SAFETY: 所有范围属于本合成文档，偏移从同一个 cookie 读取。
        let (start, end) = unsafe {
            (
                selected.CompareStart(cookie, &origin, TF_ANCHOR_START)?,
                selected.CompareEnd(cookie, &origin, TF_ANCHOR_START)?,
            )
        };
        self.doc.selection.set((start as usize, end as usize));
        Ok(())
    }
    fn GetStart(&self, _: u32) -> Result<ITfRange> {
        Ok(range(&self.doc, 0, 0))
    }
    fn GetEnd(&self, _: u32) -> Result<ITfRange> {
        let end = self.doc.text.borrow().len();
        Ok(range(&self.doc, end, end))
    }
    fn GetActiveView(&self) -> Result<ITfContextView> {
        Ok(View {
            doc: self.doc.clone(),
        }
        .into())
    }
    fn EnumViews(&self) -> Result<IEnumTfContextViews> {
        Err(E_NOTIMPL.into())
    }
    fn GetStatus(&self) -> Result<TS_STATUS> {
        Ok(TS_STATUS {
            dwDynamicFlags: if self.doc.readonly.get() {
                TS_SD_READONLY
            } else {
                0
            },
            dwStaticFlags: 0,
        })
    }
    fn GetProperty(&self, _: *const GUID) -> Result<ITfProperty> {
        Err(E_NOTIMPL.into())
    }
    fn GetAppProperty(&self, _: *const GUID) -> Result<ITfReadOnlyProperty> {
        Err(E_NOTIMPL.into())
    }
    fn TrackProperties(
        &self,
        _: *const *const GUID,
        _: u32,
        _: *const *const GUID,
        _: u32,
    ) -> Result<ITfReadOnlyProperty> {
        Err(E_NOTIMPL.into())
    }
    fn EnumProperties(&self) -> Result<IEnumTfProperties> {
        Err(E_NOTIMPL.into())
    }
    fn GetDocumentMgr(&self) -> Result<ITfDocumentMgr> {
        Err(E_NOTIMPL.into())
    }
    fn CreateRangeBackup(&self, _: u32, _: Ref<ITfRange>) -> Result<ITfRangeBackup> {
        Err(E_NOTIMPL.into())
    }
}
impl ITfContextComposition_Impl for Context_Impl {
    fn StartComposition(
        &self,
        _: u32,
        range: Ref<ITfRange>,
        sink: Ref<ITfCompositionSink>,
    ) -> Result<ITfComposition> {
        *self.doc.sink.borrow_mut() = sink.as_ref().cloned();
        Ok(Composition {
            doc: self.doc.clone(),
            range: range.as_ref().ok_or(E_INVALIDARG)?.clone(),
        }
        .into())
    }
    fn EnumCompositions(&self) -> Result<IEnumITfCompositionView> {
        Err(E_NOTIMPL.into())
    }
    fn FindComposition(&self, _: u32, _: Ref<ITfRange>) -> Result<IEnumITfCompositionView> {
        Err(E_NOTIMPL.into())
    }
    fn TakeOwnership(
        &self,
        _: u32,
        _: Ref<ITfCompositionView>,
        _: Ref<ITfCompositionSink>,
    ) -> Result<ITfComposition> {
        Err(E_NOTIMPL.into())
    }
}
impl ITfComposition_Impl for Composition_Impl {
    fn GetRange(&self) -> Result<ITfRange> {
        Ok(self.range.clone())
    }
    fn ShiftStart(&self, _: u32, _: Ref<ITfRange>) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ShiftEnd(&self, _: u32, _: Ref<ITfRange>) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn EndComposition(&self, cookie: u32) -> Result<()> {
        if self.doc.fail_end.get() {
            return Err(E_FAIL.into());
        }
        self.doc.ends.set(self.doc.ends.get() + 1);
        let sink = self.doc.sink.borrow_mut().take();
        if let Some(sink) = sink {
            // SAFETY: 只回调本测试保存的合成组合 sink，模拟系统同步重入。
            unsafe {
                sink.OnCompositionTerminated(cookie, None::<&ITfComposition>)?;
            }
        }
        Ok(())
    }
}
impl ITfRange_Impl for Range_Impl {
    fn GetText(&self, _: u32, _: u32, text: PWSTR, max: u32, count: *mut u32) -> Result<()> {
        if text.is_null() || count.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let doc = self.doc.text.borrow();
        let slice = doc.get(self.start.get()..self.end.get()).ok_or(E_FAIL)?;
        let length = slice.len().min(max as usize);
        // SAFETY: 输出容量由 COM 参数指定，长度已夹取；源 Vec 借用期间不改动。
        unsafe {
            std::ptr::copy_nonoverlapping(slice.as_ptr(), text.0, length);
            count.write(length as u32);
        }
        Ok(())
    }
    fn SetText(&self, _: u32, _: u32, text: &PCWSTR, count: i32) -> Result<()> {
        if self.doc.fail_write.get() {
            return Err(E_FAIL.into());
        }
        if count < 0 || (count != 0 && text.is_null()) {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: 调用者传入 count 个 UTF-16 单元，测试只在同步调用中复制，不保留指针。
        let text = if count == 0 {
            &[]
        } else {
            // SAFETY: count 已校验非负，COM 调用者保证该 UTF-16 缓冲区在同步调用内有效。
            unsafe { std::slice::from_raw_parts(text.0, count as usize) }
        };
        self.doc
            .text
            .borrow_mut()
            .splice(self.start.get()..self.end.get(), text.iter().copied());
        self.end.set(self.start.get() + text.len());
        self.doc.selection.set((self.end.get(), self.end.get()));
        self.doc.writes.set(self.doc.writes.get() + 1);
        Ok(())
    }
    fn IsEmpty(&self, _: u32) -> Result<BOOL> {
        Ok((self.start.get() == self.end.get()).into())
    }
    fn Collapse(&self, _: u32, anchor: TfAnchor) -> Result<()> {
        if anchor == TF_ANCHOR_END {
            self.start.set(self.end.get());
        } else {
            self.end.set(self.start.get());
        }
        Ok(())
    }
    fn CompareStart(&self, _: u32, _: Ref<ITfRange>, _: TfAnchor) -> Result<i32> {
        Ok(self.start.get() as i32)
    }
    fn CompareEnd(&self, _: u32, other: Ref<ITfRange>, anchor: TfAnchor) -> Result<i32> {
        // SAFETY: 在测试中以空原点范围归一化另一个范围的端点，不接触真实文本宿主。
        let origin = range(&self.doc, 0, 0);
        let other = other.as_ref().ok_or(E_INVALIDARG)?;
        let offset = if anchor == TF_ANCHOR_END {
            // SAFETY: 本实现的 CompareStart 取 start，选择范围已折叠，end 与 start 相同。
            unsafe { other.CompareStart(1, &origin, TF_ANCHOR_START)? }
        } else {
            0
        };
        Ok(self.end.get() as i32 - offset)
    }
    fn Clone(&self) -> Result<ITfRange> {
        Ok(range(&self.doc, self.start.get(), self.end.get()))
    }
    fn GetFormattedText(&self, _: u32) -> Result<IDataObject> {
        Err(E_NOTIMPL.into())
    }
    fn GetEmbedded(&self, _: u32, _: *const GUID, _: *const GUID) -> Result<IUnknown> {
        Err(E_NOTIMPL.into())
    }
    fn InsertEmbedded(&self, _: u32, _: u32, _: Ref<IDataObject>) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ShiftStart(&self, _: u32, _: i32, _: *mut i32, _: *const TF_HALTCOND) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ShiftEnd(&self, _: u32, _: i32, _: *mut i32, _: *const TF_HALTCOND) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ShiftStartToRange(&self, _: u32, _: Ref<ITfRange>, _: TfAnchor) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ShiftEndToRange(&self, _: u32, _: Ref<ITfRange>, _: TfAnchor) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ShiftStartRegion(&self, _: u32, _: TfShiftDir) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn ShiftEndRegion(&self, _: u32, _: TfShiftDir) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn IsEqualStart(&self, _: u32, _: Ref<ITfRange>, _: TfAnchor) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn IsEqualEnd(&self, _: u32, _: Ref<ITfRange>, _: TfAnchor) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn AdjustForInsert(&self, _: u32, _: u32) -> Result<BOOL> {
        Err(E_NOTIMPL.into())
    }
    fn GetGravity(&self, _: *mut TfGravity, _: *mut TfGravity) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn SetGravity(&self, _: u32, _: TfGravity, _: TfGravity) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetContext(&self) -> Result<ITfContext> {
        Err(E_NOTIMPL.into())
    }
}

#[test]
fn com_preedit_non_bmp_commit_next_composition_and_cancel_preserve_committed_prefix() {
    let (doc, editor) = setup();
    let report = apply(&editor, outcome(1, "u", None), deadline());
    assert!(report.error.is_none() && report.exchanged && report.handled);
    assert_eq!(text(&doc), "u");
    let report = apply(&editor, outcome(2, "a", Some("𠀀")), deadline());
    assert!(report.committed && report.error.is_none());
    assert_eq!(text(&doc), "𠀀a");
    assert_eq!(doc.selection.get(), (3, 3));
    assert!(editor.cancel(deadline()).error.is_none());
    assert_eq!(text(&doc), "𠀀");
    assert!(
        apply(&editor, outcome(3, "b", None), deadline())
            .error
            .is_none()
    );
    assert!(editor.cancel(deadline()).error.is_none());
}

#[test]
fn document_checks_precede_the_exchange_so_rejected_keys_leave_the_backend_unchanged() {
    let (doc, editor) = setup();
    let calls = Rc::new(Cell::new(0));
    let key = |editor: &Editor, deadline| {
        let calls = calls.clone();
        editor.key(deadline, move |_, _| {
            calls.set(calls.get() + 1);
            Verdict::Apply(outcome(1, "u", None))
        })
    };
    let rejected = |report: EditReport| report.error.is_some() && !report.exchanged;
    assert!(rejected(key(&editor, Instant::now())));
    doc.readonly.set(true);
    assert!(rejected(key(&editor, deadline())));
    doc.readonly.set(false);
    *doc.text.borrow_mut() = "原文".encode_utf16().collect();
    doc.selection.set((0, 2));
    assert!(rejected(key(&editor, deadline())));
    doc.selection.set((2, 2));
    doc.defer.set(true);
    assert!(rejected(key(&editor, deadline())));
    let deferred = doc.deferred.borrow_mut().take().unwrap();
    // SAFETY: 合成宿主故意违反同步约定，编辑对象仍由 COM 引用计数持有；失效令牌必须拒绝写入。
    assert!(unsafe { deferred.DoEditSession(1) }.is_err());
    doc.defer.set(false);
    editor.invalidate();
    assert!(rejected(key(&editor, deadline())));
    assert_eq!(calls.get(), 0);
    assert_eq!(doc.writes.get(), 0);
    assert_eq!(text(&doc), "原文");
}

#[test]
fn external_edits_end_the_old_composition_without_deleting_text_and_request_a_reset() {
    let (doc, editor) = setup();
    let resets = Rc::new(RefCell::new(Vec::new()));
    let key = |preedit: &'static str, commit: Option<&'static str>| {
        let resets = resets.clone();
        editor.key(deadline(), move |reset, _| {
            resets.borrow_mut().push(reset);
            Verdict::Apply(outcome(1, preedit, commit))
        })
    };
    assert!(key("u", None).error.is_none());
    doc.text.borrow_mut()[0] = 'x' as u16;
    // 宿主改写过的文字保留，本键在当前插入点照常处理。
    assert!(key("", Some("的")).error.is_none());
    assert_eq!(text(&doc), "x的");
    assert!(key("u", None).error.is_none());
    // 用户把光标移出组合：旧组合原样留给宿主，新组合从新插入点开始。
    doc.selection.set((0, 0));
    assert!(key("a", None).error.is_none());
    assert_eq!(text(&doc), "ax的u");
    assert_eq!(*resets.borrow(), [false, true, false, true]);
    assert!(editor.cancel(deadline()).error.is_none());
    assert_eq!(text(&doc), "x的u");
}

#[test]
fn host_termination_keeps_host_text_and_the_next_key_asks_for_a_reset() {
    let (doc, editor) = setup();
    assert!(
        apply(&editor, outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    assert!(editor.composing());
    let sink = doc.sink.borrow().as_ref().unwrap().clone();
    // SAFETY: 模拟宿主终止当前组合；回调只置标志，不借用编辑状态。
    unsafe {
        sink.OnCompositionTerminated(1, None::<&ITfComposition>)
            .unwrap();
    }
    assert!(!editor.composing());
    let reset = Rc::new(Cell::new(false));
    let seen = reset.clone();
    let report = editor.key(deadline(), move |value, _| {
        seen.set(value);
        Verdict::Apply(outcome(2, "a", None))
    });
    assert!(report.error.is_none() && reset.get());
    assert_eq!(text(&doc), "ua");
}

#[test]
fn exchange_failure_discards_own_preedit_and_unhandled_keys_write_nothing() {
    let (doc, editor) = setup();
    assert!(
        apply(&editor, outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    let report = editor.key(deadline(), |_, _| Verdict::Lost);
    assert!(report.exchanged && report.error.is_some() && !report.handled);
    assert_eq!(text(&doc), "");
    assert!(!editor.composing());
    let mut unhandled = outcome(2, "", None);
    unhandled.handled = false;
    let report = apply(&editor, unhandled, deadline());
    assert!(report.exchanged && report.error.is_none() && !report.handled);
    assert_eq!(doc.writes.get(), 2);
}

#[test]
fn selected_user_text_and_confirmed_commit_survive_failure() {
    let (doc, editor) = setup();
    *doc.text.borrow_mut() = "原文".encode_utf16().collect();
    doc.selection.set((2, 2));
    assert!(
        apply(&editor, outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    doc.fail_end.set(true);
    let report = apply(&editor, outcome(2, "", Some("的")), deadline());
    assert!(report.committed && report.error.is_some());
    // 清理失败期间不再交换：重试只结束旧组合，不能重放已确认提交。
    let report = apply(&editor, outcome(2, "", Some("的")), deadline());
    assert!(report.error.is_some() && !report.exchanged);
    assert!(editor.cancel(deadline()).error.is_some());
    assert_eq!(text(&doc), "原文的");
    doc.fail_end.set(false);
    assert!(editor.cancel(deadline()).error.is_none());
    assert_eq!(text(&doc), "原文的");
}

#[test]
fn failed_preedit_write_reports_the_exchange_and_can_be_cancelled() {
    let (doc, editor) = setup();
    doc.fail_write.set(true);
    let report = apply(&editor, outcome(1, "u", None), deadline());
    assert!(report.exchanged && report.error.is_some() && !report.committed);
    assert!(doc.text.borrow().is_empty());
    doc.fail_write.set(false);
    assert!(editor.cancel(deadline()).error.is_none());
    assert!(
        apply(&editor, outcome(2, "a", None), deadline())
            .error
            .is_none()
    );
    assert_eq!(text(&doc), "a");
}

#[test]
fn old_composition_termination_cannot_invalidate_a_new_composition() {
    let (doc, editor) = setup();
    assert!(
        apply(&editor, outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    let old_sink = doc.sink.borrow().as_ref().unwrap().clone();
    assert!(
        apply(&editor, outcome(2, "a", Some("的")), deadline())
            .error
            .is_none()
    );
    // SAFETY: 故意迟到调用旧的 COM sink，必须按组合代次丢弃，不能影响新组合。
    unsafe {
        old_sink
            .OnCompositionTerminated(1, None::<&ITfComposition>)
            .unwrap();
    }
    let reset = Rc::new(Cell::new(true));
    let seen = reset.clone();
    let report = editor.key(deadline(), move |value, _| {
        seen.set(value);
        Verdict::Apply(outcome(3, "ab", None))
    });
    assert!(report.error.is_none() && !reset.get());
    assert_eq!(text(&doc), "的ab");
}

#[test]
fn abandon_cancels_now_or_queues_one_verified_cancel_and_blocks_later_keys() {
    let (doc, context) = document();
    let editor = Editor::new(context.clone(), 1, 1, || {}).unwrap();
    assert!(editor.is(&context) && !editor.is(&document().1));
    assert!(
        apply(&editor, outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    editor.abandon();
    assert_eq!(text(&doc), "");
    assert!(
        apply(&editor, outcome(2, "a", None), deadline())
            .error
            .is_some()
    );
    // 拿不到同步锁：排队的取消仍只删除与记录一致的自有组合。
    for external in [false, true] {
        let (doc, editor) = setup();
        assert!(
            apply(&editor, outcome(1, "u", None), deadline())
                .error
                .is_none()
        );
        doc.defer.set(true);
        editor.abandon();
        let queued = doc.deferred.borrow_mut().take().unwrap();
        if external {
            doc.text.borrow_mut()[0] = 'x' as u16;
        }
        // SAFETY: 由合成宿主稍后执行异步会话，编辑对象仍由 COM 引用计数持有。
        let result = unsafe { queued.DoEditSession(1) };
        assert_eq!(result.is_ok(), !external);
        assert_eq!(text(&doc), if external { "x" } else { "" });
    }
}

#[test]
fn key_sessions_report_the_composition_rectangle_only_while_composing() {
    let (doc, editor) = setup();
    let report = apply(&editor, outcome(1, "un", None), deadline());
    let expected = RECT {
        left: 100,
        top: 100,
        right: 120,
        bottom: 120,
    };
    assert_eq!(
        (report.caret, report.owner),
        (Some(expected), HWND::default())
    );
    // 宿主未完成布局：写入照常成功，只是没有位置，由候选窗另行回退。
    doc.no_layout.set(true);
    let report = apply(&editor, outcome(2, "uni", None), deadline());
    assert!(report.error.is_none() && report.handled && report.caret.is_none());
    doc.no_layout.set(false);
    let report = apply(&editor, outcome(3, "", Some("的")), deadline());
    assert!(report.committed && report.caret.is_none());
}

type Shared<T> = Rc<RefCell<T>>;
fn select(
    editor: &Editor,
    verdict: impl FnOnce() -> Verdict + 'static,
) -> (bool, Shared<Option<EditReport>>, Rc<Cell<bool>>) {
    let (report, called) = (Rc::new(RefCell::new(None)), Rc::new(Cell::new(false)));
    let (sink, seen) = (report.clone(), called.clone());
    let queued = editor.select(
        move |_| {
            seen.set(true);
            verdict()
        },
        move |value| *sink.borrow_mut() = Some(value),
    );
    (queued, report, called)
}

#[test]
fn click_sessions_commit_into_the_intact_composition_and_never_end_or_reset_it() {
    let (doc, editor) = setup();
    // 没有自有组合：不交换，报告失败。
    let (queued, report, called) = select(&editor, || Verdict::Apply(outcome(1, "", Some("的"))));
    assert!(queued && !called.get());
    assert!(report.borrow().as_ref().unwrap().error.is_some());
    assert!(
        apply(&editor, outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    // 后台明确未处理：不写入、不作废组合。
    let (_, report, called) = select(&editor, || Verdict::Skip);
    let skipped = report.borrow_mut().take().unwrap();
    assert!(called.get() && !skipped.exchanged && skipped.error.is_some());
    assert!(editor.composing() && text(&doc) == "u");
    // 用户把光标移走不影响点击（组合文本未变）；外部改写则拒绝且不结束组合。
    doc.selection.set((0, 0));
    doc.text.borrow_mut()[0] = 'x' as u16;
    let (_, report, called) = select(&editor, || Verdict::Apply(outcome(2, "", Some("的"))));
    assert!(!called.get() && report.borrow().as_ref().unwrap().error.is_some());
    assert_eq!(text(&doc), "x");
    doc.text.borrow_mut()[0] = 'u' as u16;
    let (_, report, _) = select(&editor, || Verdict::Apply(outcome(3, "", Some("的"))));
    let done = report.borrow_mut().take().unwrap();
    assert!(done.committed && done.handled && done.error.is_none());
    assert_eq!(text(&doc), "的");
    assert!(!editor.composing());
}

#[test]
fn click_on_a_terminated_composition_keeps_the_reset_for_the_next_key() {
    let (doc, editor) = setup();
    assert!(
        apply(&editor, outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    let sink = doc.sink.borrow().as_ref().unwrap().clone();
    // SAFETY: 模拟宿主终止当前组合。
    unsafe {
        sink.OnCompositionTerminated(1, None::<&ITfComposition>)
            .unwrap();
    }
    let (_, report, called) = select(&editor, || Verdict::Apply(outcome(2, "", Some("的"))));
    assert!(!called.get() && report.borrow().as_ref().unwrap().error.is_some());
    let reset = Rc::new(Cell::new(false));
    let seen = reset.clone();
    let report = editor.key(deadline(), move |value, _| {
        seen.set(value);
        Verdict::Apply(outcome(3, "a", None))
    });
    assert!(report.error.is_none() && reset.get());
    assert_eq!(text(&doc), "ua");
}

#[test]
fn queued_click_budget_starts_when_the_session_runs_and_invalidation_cancels_it() {
    for invalidate in [false, true] {
        let (doc, editor) = setup();
        assert!(
            apply(&editor, outcome(1, "u", None), deadline())
                .error
                .is_none()
        );
        doc.defer.set(true);
        let window = Rc::new(Cell::new(None));
        let seen = window.clone();
        let finished = Rc::new(Cell::new(false));
        let done = finished.clone();
        assert!(editor.select(
            move |deadline| {
                seen.set(Some(deadline.saturating_duration_since(Instant::now())));
                Verdict::Apply(outcome(2, "", Some("的")))
            },
            move |_| done.set(true),
        ));
        assert!(!finished.get());
        if invalidate {
            editor.invalidate();
        }
        let queued = doc.deferred.borrow_mut().take().unwrap();
        let started = Instant::now();
        // SAFETY: 合成宿主稍后执行排队会话，编辑对象仍由 COM 引用计数持有。
        let result = unsafe { queued.DoEditSession(1) };
        assert_eq!(result.is_ok(), !invalidate);
        if invalidate {
            // 失效的排队会话在访问文档前拒绝，也不回调结束通知。
            assert!(window.get().is_none() && !finished.get());
            assert_eq!(text(&doc), "u");
        } else {
            let budget = window.get().unwrap();
            assert!(
                budget <= Duration::from_millis(50) && started.elapsed() < Duration::from_secs(1)
            );
            assert!(finished.get());
            assert_eq!(text(&doc), "的");
        }
    }
}

#[test]
fn host_termination_notifies_once_and_stale_composition_sinks_stay_silent() {
    let (doc, context) = document();
    let count = Rc::new(Cell::new(0));
    let seen = count.clone();
    let editor = Editor::new(context, 1, 1, move || seen.set(seen.get() + 1)).unwrap();
    assert!(
        apply(&editor, outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    assert_eq!(count.get(), 0);
    let old = doc.sink.borrow().as_ref().unwrap().clone();
    // SAFETY: 模拟宿主终止当前组合。
    unsafe { old.OnCompositionTerminated(1, None::<&ITfComposition>) }.unwrap();
    assert_eq!(count.get(), 1);
    assert!(
        apply(&editor, outcome(2, "a", None), deadline())
            .error
            .is_none()
    );
    // SAFETY: 旧组合的迟到通知按代次丢弃，不再收起新组合的候选窗。
    unsafe { old.OnCompositionTerminated(1, None::<&ITfComposition>) }.unwrap();
    assert_eq!(count.get(), 1);
}
