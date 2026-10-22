//! 真正 COM 接口/vtable 的合成文本宿主；不激活系统输入法、不碰剪贴板或用户窗口。
use super::*;
use std::time::Duration;
use windows::{
    Win32::{Foundation::E_NOTIMPL, System::Com::IDataObject},
    core::{BOOL, GUID, IUnknown, PCWSTR, PWSTR},
};

#[derive(Default)]
struct Document {
    text: RefCell<Vec<u16>>,
    selection: Cell<(usize, usize)>,
    sink: RefCell<Option<ITfCompositionSink>>,
    readonly: Cell<bool>,
    fail_end: Cell<bool>,
    fail_write: Cell<bool>,
    writes: Cell<u32>,
    ends: Cell<u32>,
    defer: Cell<bool>,
    deferred: RefCell<Option<ITfEditSession>>,
}
#[implement(ITfContext, ITfContextComposition)]
struct Context {
    doc: Rc<Document>,
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
        state: tigerrime_contracts::InputState {
            revision,
            composing: !preedit.is_empty(),
            composition: tigerrime_contracts::Composition {
                text: preedit.into(),
                cursor: preedit.len(),
                selection: 0..preedit.len(),
            },
            ..Default::default()
        },
    }
}
fn setup() -> (Rc<Document>, Editor) {
    let doc = Rc::new(Document::default());
    let context: ITfContext = Context { doc: doc.clone() }.into();
    (doc, Editor::new(context, 1, 1).unwrap())
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
        Err(E_NOTIMPL.into())
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
    assert!(
        editor
            .apply(outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    assert_eq!(String::from_utf16(&doc.text.borrow()).unwrap(), "u");
    let report = editor.apply(outcome(2, "a", Some("𠀀")), deadline());
    assert!(report.committed && report.error.is_none());
    assert_eq!(String::from_utf16(&doc.text.borrow()).unwrap(), "𠀀a");
    assert_eq!(doc.selection.get(), (3, 3));
    assert!(editor.cancel(deadline()).error.is_none());
    assert_eq!(String::from_utf16(&doc.text.borrow()).unwrap(), "𠀀");
    assert!(
        editor
            .apply(outcome(3, "b", None), deadline())
            .error
            .is_none()
    );
    assert!(editor.cancel(deadline()).error.is_none());
}
#[test]
fn expired_duplicate_blurred_readonly_and_external_edits_are_rejected() {
    let (doc, editor) = setup();
    assert!(
        editor
            .apply(outcome(1, "u", None), Instant::now())
            .error
            .is_some()
    );
    assert_eq!(doc.writes.get(), 0);
    doc.readonly.set(true);
    assert!(
        editor
            .apply(outcome(1, "u", None), deadline())
            .error
            .is_some()
    );
    doc.readonly.set(false);
    assert!(
        editor
            .apply(outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    assert!(
        editor
            .apply(outcome(1, "u", Some("的")), deadline())
            .error
            .is_some()
    );
    doc.text.borrow_mut()[0] = 'x' as u16;
    assert!(
        editor
            .apply(outcome(2, "", Some("的")), deadline())
            .error
            .is_some()
    );
    assert!(editor.cancel(deadline()).error.is_some());
    assert_eq!(String::from_utf16(&doc.text.borrow()).unwrap(), "x");
    editor.invalidate();
    assert!(
        editor
            .apply(outcome(3, "a", None), deadline())
            .error
            .is_some()
    );
}
#[test]
fn selected_user_text_and_confirmed_commit_survive_failure() {
    let (doc, editor) = setup();
    *doc.text.borrow_mut() = "原文".encode_utf16().collect();
    doc.selection.set((0, 2));
    assert!(
        editor
            .apply(outcome(1, "u", None), deadline())
            .error
            .is_some()
    );
    assert_eq!(String::from_utf16(&doc.text.borrow()).unwrap(), "原文");
    doc.selection.set((2, 2));
    assert!(
        editor
            .apply(outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    doc.fail_end.set(true);
    let report = editor.apply(outcome(2, "", Some("的")), deadline());
    assert!(report.committed && report.error.is_some());
    assert!(
        editor
            .apply(outcome(2, "", Some("的")), deadline())
            .error
            .is_some()
    );
    assert!(editor.cancel(deadline()).error.is_some());
    assert_eq!(String::from_utf16(&doc.text.borrow()).unwrap(), "原文的");
    doc.fail_end.set(false);
    assert!(editor.cancel(deadline()).error.is_none());
    assert_eq!(String::from_utf16(&doc.text.borrow()).unwrap(), "原文的");
}

#[test]
fn failed_preedit_write_is_not_replayed_and_can_be_cancelled() {
    let (doc, editor) = setup();
    doc.fail_write.set(true);
    let report = editor.apply(outcome(1, "u", None), deadline());
    assert!(report.error.is_some() && !report.committed);
    assert!(doc.text.borrow().is_empty());
    doc.fail_write.set(false);
    assert!(editor.cancel(deadline()).error.is_none());
    assert!(
        editor
            .apply(outcome(1, "u", None), deadline())
            .error
            .is_some()
    );
    assert!(
        editor
            .apply(outcome(2, "a", None), deadline())
            .error
            .is_none()
    );
}

#[test]
fn queued_com_callback_after_return_cannot_apply_a_late_commit() {
    let (doc, editor) = setup();
    doc.defer.set(true);
    assert!(
        editor
            .apply(outcome(1, "", Some("的")), deadline())
            .error
            .is_some()
    );
    let deferred = doc.deferred.borrow_mut().take().unwrap();
    // SAFETY: 合成宿主故意违反同步约定，编辑对象仍由 COM 引用计数持有；失效令牌必须拒绝写入。
    assert!(unsafe { deferred.DoEditSession(1) }.is_err());
    assert!(doc.text.borrow().is_empty());
    assert_eq!(doc.writes.get(), 0);
}

#[test]
fn old_composition_termination_cannot_invalidate_a_new_composition() {
    let (doc, editor) = setup();
    assert!(
        editor
            .apply(outcome(1, "u", None), deadline())
            .error
            .is_none()
    );
    let old_sink = doc.sink.borrow().as_ref().unwrap().clone();
    assert!(
        editor
            .apply(outcome(2, "a", Some("的")), deadline())
            .error
            .is_none()
    );
    // SAFETY: 故意迟到调用旧的 COM sink，必须按组合代次丢弃，不能影响新组合。
    unsafe {
        old_sink
            .OnCompositionTerminated(1, None::<&ITfComposition>)
            .unwrap();
    }
    assert!(
        editor
            .apply(outcome(3, "ab", None), deadline())
            .error
            .is_none()
    );
    assert_eq!(String::from_utf16(&doc.text.borrow()).unwrap(), "的ab");
}
