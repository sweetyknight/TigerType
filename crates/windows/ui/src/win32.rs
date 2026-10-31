//! Windows 原生视图组装。内部依赖由上到下：events → panels/window/tray → controls → drawing/modal。
//! 候选窗在 input-method；本模块不访问引擎或文件事务。各资源仍在所属 UI 线程配对释放。
use crate::{Inbox, Latest};
use std::{
    cell::Cell,
    ffi::c_void,
    io,
    os::windows::ffi::OsStrExt,
    os::windows::io::AsRawHandle,
    path::Path,
    ptr::{null, null_mut},
    sync::mpsc::{SyncSender, TryRecvError},
};
#[cfg(test)]
use tigertype_contracts::OutputForm;
use tigertype_contracts::{
    CandidateFont, CustomColors, Options, Palette, Theme, TypingMode,
    protocol::Panel,
    ui::{Reply, Request},
};
use tigertype_ui_model::{
    layout::*,
    model::{Effect, Model, details},
};
use windows_sys::Win32::{
    Foundation::{
        COLORREF, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, POINT, RECT, SIZE,
        WAIT_FAILED, WPARAM,
    },
    Graphics::{
        Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
            DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
        },
        Gdi::*,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        SystemServices::{SS_CENTER, SS_CENTERIMAGE, SS_NOPREFIX, SS_OWNERDRAW, SS_PATHELLIPSIS},
        Threading::{
            INFINITE, RegisterWaitForSingleObject, SetEvent, UnregisterWaitEx, WT_EXECUTEONLYONCE,
        },
    },
    UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        Controls::{
            BST_CHECKED, BST_UNCHECKED, DRAWITEMSTRUCT, EM_GETLINE, EM_GETRECT, EM_GETSEL,
            EM_LINEFROMCHAR, EM_LINEINDEX, EM_LINELENGTH, EM_POSFROMCHAR, EM_SETLIMITTEXT,
            EM_SETMARGINS, EM_SETSEL, MEASUREITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_NOFOCUSRECT,
            ODS_SELECTED, ODT_LISTBOX, ODT_MENU, ODT_STATIC, SetScrollInfo,
        },
        HiDpi::*,
        Input::{
            Ime::{GCS_COMPSTR, ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext},
            KeyboardAndMouse::{
                EnableWindow, GetFocus, GetKeyState, IsWindowEnabled, SetFocus, VK_CONTROL,
                VK_ESCAPE, VK_MENU, VK_RETURN,
            },
        },
        Shell::ShellExecuteW,
        WindowsAndMessaging::*,
    },
};

mod controls;
mod drawing;
mod events;
mod modal;
mod panels;
mod seal;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
mod tray;
mod window;
pub use events::run;
pub use seal::ico_file;

const ACTION: u32 = WM_APP + 1;
const CLOSE: u32 = WM_APP + 2;
const LAYOUT: u32 = WM_APP + 3;
/// 收起系统模态循环：唤起邮箱在模态期间有信号（含后台停止）时由线程池回调投递给进入模态的窗口。
const UNBLOCK: u32 = WM_APP + 6;

const UI_FACE: &str = "Microsoft YaHei UI";
/// 候选字体的 Windows 字体名；未安装时由 create_font 回退到界面字体。
const fn font_face(font: CandidateFont) -> &'static str {
    match font {
        CandidateFont::WenKai => "LXGW WenKai GB Screen",
        CandidateFont::KaiTi => "KaiTi",
    }
}
const MONO_FACES: [&str; 2] = ["Cascadia Mono", "Consolas"];
/// 拆分只用启动时私有加载的锁定文楷，不跟随候选字体选项或系统回退。
const LEARNING_FACES: [&str; 1] = [tigertype_windows_fonts::FAMILY];
/// 图标字体：Win11 为 Fluent，Win10 为 MDL2，常用码位相同。
const ICON_FACES: [&str; 2] = ["Segoe Fluent Icons", "Segoe MDL2 Assets"];
/// 字块列表的行高与最小列宽（96 DPI）。
const ITEM_HEIGHT: u32 = 40;
const CHIP: i32 = 42;
