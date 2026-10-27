//! 候选窗 UIA：按 IME 约定，根元素 AutomationId 为 `IME_Candidate_Window`，候选项 Name 为候选字、
//! 高亮项为选中项。以 COM 线程模型声明，只在宿主 TSF 线程（单线程套间）上被调用；
//! 元素以弱引用指向候选窗，展示更新后旧候选项失效，不能把旧内容报告成新内容。
// 模式匹配直接使用 windows 绑定的系统常量名。
#![allow(non_upper_case_globals)]
use crate::candidate::Popup;
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    rc::{Rc, Weak},
};
use windows::{
    Win32::{
        Foundation::{E_OUTOFMEMORY, E_UNEXPECTED},
        System::{
            Com::SAFEARRAY,
            Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement},
            Variant::{VARIANT, VT_I4},
        },
        UI::Accessibility::*,
    },
    core::{Error, HRESULT, IUnknown, IUnknownImpl, Interface, Result, implement},
};

pub(crate) const AUTOMATION_ID: &str = "IME_Candidate_Window";

fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(E_UNEXPECTED.into()))
}
/// S_OK 且不写出参，表示没有该元素/模式；UIA 与 COM 存根调用前已将出参置空。
fn none<T>() -> Result<T> {
    Err(Error::empty())
}
fn gone() -> Error {
    HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32).into()
}
fn options() -> ProviderOptions {
    ProviderOptions(ProviderOptions_ServerSideProvider.0 | ProviderOptions_UseComThreading.0)
}
fn runtime_id(parts: [i32; 3]) -> Result<*mut SAFEARRAY> {
    // SAFETY: 新建一维 VT_I4 数组并逐个写入；失败时销毁，成功时所有权交给 UIA。
    unsafe {
        let array = SafeArrayCreateVector(VT_I4, 0, parts.len() as u32);
        if array.is_null() {
            return Err(E_OUTOFMEMORY.into());
        }
        for (index, part) in parts.iter().enumerate() {
            let index = index as i32;
            if let Err(error) = SafeArrayPutElement(array, &index, (part as *const i32).cast()) {
                let _ = SafeArrayDestroy(array);
                return Err(error);
            }
        }
        Ok(array)
    }
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot
)]
struct Root(Weak<Popup>);
#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    ISelectionItemProvider
)]
struct Item {
    popup: Weak<Popup>,
    serial: u32,
    index: usize,
}

pub(crate) fn root(popup: &Rc<Popup>) -> IRawElementProviderSimple {
    Root(Rc::downgrade(popup)).into()
}
/// 当前展示中的第 index 项；元素绑定展示编号，更新后即失效。
pub(crate) fn item(popup: &Rc<Popup>, index: usize) -> Option<IRawElementProviderSimple> {
    if !popup.alive() {
        return None;
    }
    let view = popup.view.try_borrow().ok()?;
    (index < view.state.as_ref()?.candidates.len()).then(|| {
        Item {
            popup: Rc::downgrade(popup),
            serial: view.serial,
            index,
        }
        .into()
    })
}
fn fragment(popup: &Rc<Popup>, index: usize) -> Result<IRawElementProviderFragment> {
    item(popup, index).ok_or_else(gone)?.cast()
}
fn count(popup: &Popup) -> usize {
    popup.view.try_borrow().map_or(0, |view| {
        view.state
            .as_ref()
            .map_or(0, |state| state.candidates.len())
    })
}

impl IRawElementProviderSimple_Impl for Root_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(options())
    }
    fn GetPatternProvider(&self, _: UIA_PATTERN_ID) -> Result<IUnknown> {
        none()
    }
    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        Ok(match id {
            UIA_AutomationIdPropertyId => VARIANT::from(AUTOMATION_ID),
            UIA_ControlTypePropertyId => VARIANT::from(UIA_ListControlTypeId.0),
            UIA_NamePropertyId => VARIANT::from("候选"),
            UIA_IsKeyboardFocusablePropertyId => VARIANT::from(false),
            _ => VARIANT::default(),
        })
    }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        guarded(|| {
            let popup = self
                .0
                .upgrade()
                .filter(|popup| popup.alive())
                .ok_or_else(gone)?;
            // SAFETY: 窗口属本线程且仍存活，宿主提供者补充窗口级属性与运行时编号。
            unsafe { UiaHostProviderFromHwnd(popup.hwnd) }
        })
    }
}
impl IRawElementProviderFragment_Impl for Root_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        guarded(|| {
            let popup = self
                .0
                .upgrade()
                .filter(|popup| popup.alive())
                .ok_or_else(gone)?;
            let count = count(&popup);
            if count == 0 {
                return none();
            }
            match direction {
                NavigateDirection_FirstChild => fragment(&popup, 0),
                NavigateDirection_LastChild => fragment(&popup, count - 1),
                _ => none(),
            }
        })
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        // 窗口根元素的运行时编号由宿主提供者给出。
        Ok(ptr::null_mut())
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        Ok(UiaRect::default())
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(ptr::null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        // 候选窗永不取得键盘焦点，焦点留在宿主组合。
        Ok(())
    }
    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(self.to_interface())
    }
}
impl IRawElementProviderFragmentRoot_Impl for Root_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        guarded(|| {
            let popup = self
                .0
                .upgrade()
                .filter(|popup| popup.alive())
                .ok_or_else(gone)?;
            match popup.hit_screen(x as i32, y as i32) {
                Some(index) => fragment(&popup, index),
                None => Ok(self.to_interface()),
            }
        })
    }
    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        none()
    }
}

impl Item_Impl {
    /// 候选窗仍在且展示未更新时才可访问；否则报告元素已不可用。
    fn live(&self) -> Result<Rc<Popup>> {
        // Rc 可能晚于系统窗口释放；必须同时验证 HWND 的登记身份。
        let popup = self
            .popup
            .upgrade()
            .filter(|popup| popup.alive())
            .ok_or_else(gone)?;
        let current = popup.view.try_borrow().is_ok_and(|view| {
            view.serial == self.serial
                && view
                    .state
                    .as_ref()
                    .is_some_and(|state| self.index < state.candidates.len())
        });
        if current { Ok(popup) } else { Err(gone()) }
    }
    fn read<T>(&self, f: impl FnOnce(&Popup, &crate::candidate::View) -> T) -> Result<T> {
        let popup = self.live()?;
        let view = popup.view.try_borrow().map_err(|_| gone())?;
        Ok(f(&popup, &view))
    }
}
impl IRawElementProviderSimple_Impl for Item_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(options())
    }
    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        if id == UIA_SelectionItemPatternId {
            Ok(self.to_interface::<ISelectionItemProvider>().cast()?)
        } else {
            none()
        }
    }
    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        guarded(|| {
            self.read(|_, view| {
                let Some(candidate) = view
                    .state
                    .as_ref()
                    .and_then(|state| state.candidates.get(self.index))
                else {
                    return VARIANT::default();
                };
                match id {
                    UIA_NamePropertyId => VARIANT::from(candidate.text.as_str()),
                    // 编码等学习提示作为帮助文本，读屏器在名称之后朗读。
                    UIA_HelpTextPropertyId if !candidate.comment.is_empty() => {
                        VARIANT::from(candidate.comment.as_str())
                    }
                    UIA_ControlTypePropertyId => VARIANT::from(UIA_ListItemControlTypeId.0),
                    UIA_IsKeyboardFocusablePropertyId => VARIANT::from(false),
                    _ => VARIANT::default(),
                }
            })
        })
    }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        none()
    }
}
impl IRawElementProviderFragment_Impl for Item_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        guarded(|| {
            let popup = self.live()?;
            match direction {
                NavigateDirection_Parent => popup.root().cast(),
                NavigateDirection_NextSibling if self.index + 1 < count(&popup) => {
                    fragment(&popup, self.index + 1)
                }
                NavigateDirection_PreviousSibling if self.index > 0 => {
                    fragment(&popup, self.index - 1)
                }
                _ => none(),
            }
        })
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        guarded(|| {
            self.live()?;
            // 展示编号参与运行时编号，翻页或更新后的同位置元素不会被当作同一元素。
            runtime_id([
                UiaAppendRuntimeId as i32,
                self.serial as i32,
                self.index as i32,
            ])
        })
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        guarded(|| {
            let rect = self.read(|popup, view| {
                view.layout
                    .rows
                    .get(self.index)
                    .map(|rect| popup.screen(*rect))
            })?;
            let rect = rect.ok_or_else(gone)?;
            Ok(UiaRect {
                left: rect.left.into(),
                top: rect.top.into(),
                width: (rect.right - rect.left).into(),
                height: (rect.bottom - rect.top).into(),
            })
        })
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(ptr::null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        Ok(())
    }
    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        guarded(|| self.live()?.root().cast())
    }
}
impl ISelectionItemProvider_Impl for Item_Impl {
    // 选字必须经编辑会话与后台快照核对，不允许通过 UIA 选择直接改写文档。
    fn Select(&self) -> Result<()> {
        Err(HRESULT(UIA_E_INVALIDOPERATION as i32).into())
    }
    fn AddToSelection(&self) -> Result<()> {
        Err(HRESULT(UIA_E_INVALIDOPERATION as i32).into())
    }
    fn RemoveFromSelection(&self) -> Result<()> {
        Err(HRESULT(UIA_E_INVALIDOPERATION as i32).into())
    }
    fn IsSelected(&self) -> Result<windows::core::BOOL> {
        guarded(|| {
            self.read(|_, view| {
                view.state
                    .as_ref()
                    .is_some_and(|state| state.highlighted == Some(self.index))
                    .into()
            })
        })
    }
    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        guarded(|| Ok(self.live()?.root()))
    }
}
