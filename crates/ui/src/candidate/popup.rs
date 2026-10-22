//! 不激活宿主的原生候选窗；标准按钮保留系统可访问性，通知只携带整数展示代次。
use super::{FrameInbox, View, position};
use crate::{ReplySender, native::DpiContext};
use std::{
    os::windows::io::AsRawHandle,
    ptr::{null, null_mut},
    sync::mpsc::TryRecvError,
};
use tigertype_contracts::candidate::{CandidateChoice, CandidateFrame, ScreenRect};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WAIT_FAILED, WPARAM},
    Graphics::Gdi::{
        CLEARTYPE_QUALITY, COLOR_WINDOW, CreateFontW, DEFAULT_CHARSET, DeleteObject,
        GetMonitorInfoW, GetSysColorBrush, HFONT, MONITOR_DEFAULTTONEAREST, MONITORINFO,
        MonitorFromRect,
    },
    System::{LibraryLoader::GetModuleHandleW, Threading::INFINITE},
    UI::{HiDpi::GetDpiForWindow, Input::KeyboardAndMouse::GetCapture, WindowsAndMessaging::*},
};
const CHOOSE: u32 = WM_APP + 20;
const CLOSE: u32 = WM_APP + 21;
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn error() -> String {
    std::io::Error::last_os_error().to_string()
}

// SAFETY: 回调只操作本线程窗口和整数通知，无业务指针、分配或可能展开的 Rust 代码。
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: Windows 提供有效窗口；展示代次由所属 UI 线程明确写入整数槽。
    unsafe {
        match message {
            WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
            WM_COMMAND if w == 10 => {
                if PostMessageW(hwnd, CLOSE, 0, 0) == 0 {
                    PostQuitMessage(1);
                }
                0
            }
            WM_COMMAND if w >> 16 == BN_CLICKED as usize && (1..=9).contains(&(w & 0xffff)) => {
                if PostMessageW(
                    hwnd,
                    CHOOSE,
                    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as usize,
                    ((w & 0xffff) - 1) as isize,
                ) == 0
                {
                    PostQuitMessage(1);
                }
                0
            }
            WM_CLOSE => {
                if PostMessageW(hwnd, CLOSE, 0, 0) == 0 {
                    PostQuitMessage(1);
                }
                0
            }
            _ => DefWindowProcW(hwnd, message, w, l),
        }
    }
}
struct Popup {
    hwnd: HWND,
    controls: [HWND; 11],
    font: HFONT,
    dpi: u32,
    class: Vec<u16>,
    close_label: String,
}
impl Popup {
    fn new(close_label: &str) -> Result<Self, String> {
        let class = wide("TigerType.Candidate.Preview.v1");
        // SAFETY: 注册无状态回调；画刷与光标为系统共享对象。
        if unsafe {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: GetModuleHandleW(null()),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: GetSysColorBrush(COLOR_WINDOW),
                lpszClassName: class.as_ptr(),
                ..Default::default()
            })
        } == 0
        {
            return Err(error());
        }
        let mut popup = Self {
            hwnd: null_mut(),
            controls: [null_mut(); 11],
            font: null_mut(),
            dpi: 0,
            class,
            close_label: close_label.into(),
        };
        // SAFETY: 显式 NOACTIVATE/TOOLWINDOW，不抢宿主焦点、不在任务栏建立普通应用入口。
        popup.hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                popup.class.as_ptr(),
                wide("TigerType 候选").as_ptr(),
                WS_POPUP | WS_BORDER,
                0,
                0,
                420,
                400,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            )
        };
        if popup.hwnd.is_null() {
            return Err(error());
        }
        for id in 0..11 {
            let class = wide(if id == 0 { "STATIC" } else { "BUTTON" });
            // SAFETY: 子控件由父窗配对销毁；整数 ID 与固定数组一致，未传入业务指针。
            popup.controls[id] = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    wide("").as_ptr(),
                    WS_CHILD | WS_VISIBLE | if id == 0 { 0 } else { BS_LEFT as u32 },
                    0,
                    0,
                    0,
                    0,
                    popup.hwnd,
                    id as _,
                    GetModuleHandleW(null()),
                    null(),
                )
            };
            if popup.controls[id].is_null() {
                return Err(error());
            }
        }
        Ok(popup)
    }
    fn font(&mut self) -> Result<(), String> {
        // SAFETY: 窗口在 UI 线程存活，系统返回当前 DPI。
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96);
        if dpi == self.dpi {
            return Ok(());
        }
        // SAFETY: 新字体先替换全部控件，再释放旧字体，避免控件引用已释放 GDI 对象。
        let font = unsafe {
            CreateFontW(
                -(15 * dpi as i32 / 72),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                wide("Microsoft YaHei UI").as_ptr(),
            )
        };
        if font.is_null() {
            return Err(error());
        }
        for hwnd in self.controls {
            // SAFETY: font 在 Popup 中持有至所有控件销毁。
            unsafe {
                SendMessageW(hwnd, WM_SETFONT, font as usize, 1);
            }
        }
        if !self.font.is_null() {
            // SAFETY: 旧字体已经不被控件使用。
            unsafe {
                DeleteObject(self.font);
            }
        }
        self.font = font;
        self.dpi = dpi;
        Ok(())
    }
    fn hide(&self) {
        // SAFETY: 仅隐藏自己持有的候选窗，保留宿主焦点。
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }
    fn display(
        &mut self,
        frame: Option<&CandidateFrame>,
        serial: u32,
        visible: bool,
    ) -> Result<(), String> {
        // SAFETY: 更新前取消旧按钮的鼠标捕获，防止按下旧候选、抬起时选中新候选。
        unsafe {
            let capture = GetCapture();
            if self.controls.contains(&capture) {
                SendMessageW(capture, WM_CANCELMODE, 0, 0);
            }
            // x86 的 Ptr API 是 i32 别名；只存 u32 位型，读取时同样恢复为 u32。
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, serial as _);
        }
        let Some(frame) = frame.filter(|f| f.state.composing && !f.state.candidates.is_empty())
        else {
            self.hide();
            return Ok(());
        };
        let rect = RECT {
            left: frame.caret.left,
            top: frame.caret.top,
            right: frame.caret.right,
            bottom: frame.caret.bottom,
        };
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: RECT 是值类型屏幕位置，MONITORINFO 输出容量匹配。
        if unsafe {
            GetMonitorInfoW(
                MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
        } == 0
        {
            return Err(error());
        }
        // 先移动到目标显示器，再读取窗口 DPI；不会激活或改变宿主文字。
        // SAFETY: 使用本窗口句柄与有界协议坐标，不传递跨线程 HWND。
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                frame.caret.left,
                frame.caret.bottom,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        self.font()?;
        let scale = |n| n * self.dpi as i32 / 96;
        let work = ScreenRect {
            left: monitor.rcWork.left,
            top: monitor.rcWork.top,
            right: monitor.rcWork.right,
            bottom: monitor.rcWork.bottom,
        };
        let location = position(
            frame.caret,
            work,
            scale(420),
            scale(78 + frame.state.candidates.len() as i32 * 38),
        );
        // SAFETY: 工作区夹取后的尺寸不会越过监视器；SWP_NOACTIVATE 保持输入宿主焦点。
        unsafe {
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                location.left,
                location.top,
                location.right - location.left,
                location.bottom - location.top,
                SWP_NOACTIVATE,
            );
        }
        for (index, hwnd) in self.controls.iter().copied().enumerate() {
            let text = if index == 0 {
                format!("候选：{}", frame.state.composition.text)
            } else if index == 10 {
                self.close_label.clone()
            } else if let Some(candidate) = frame.state.candidates.get(index - 1) {
                let value = format!(
                    "{} {} {} {}",
                    if frame.state.highlighted == Some(index - 1) {
                        "▶"
                    } else {
                        " "
                    },
                    index,
                    candidate.text,
                    candidate.comment
                );
                // 长注释明确省略；完整学习资料仍在独立查字窗，不让单行撑出工作区。
                if value.chars().count() > 80 {
                    value.chars().take(80).collect::<String>() + "…"
                } else {
                    value
                }
            } else {
                String::new()
            };
            // SAFETY: 文本在同步调用期间存活；只有当前页的按钮可见。
            unsafe {
                SetWindowTextW(hwnd, wide(&text).as_ptr());
                MoveWindow(
                    hwnd,
                    scale(6),
                    scale(if index == 0 {
                        4
                    } else if index == 10 {
                        34 + frame.state.candidates.len() as i32 * 38
                    } else {
                        34 + (index as i32 - 1) * 38
                    }),
                    location.right - location.left - scale(14),
                    scale(if index == 0 { 28 } else { 36 }),
                    1,
                );
                ShowWindow(
                    hwnd,
                    if index == 10 || index <= frame.state.candidates.len() {
                        SW_SHOWNA
                    } else {
                        SW_HIDE
                    },
                );
            }
        }
        if visible {
            // SAFETY: SW_SHOWNOACTIVATE 不改变原宿主的激活与键盘焦点。
            unsafe {
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
        }
        Ok(())
    }
}
impl Drop for Popup {
    fn drop(&mut self) {
        // SAFETY: 先销毁子控件，再回收字体与类；包含创建中途失败路径。
        unsafe {
            if !self.hwnd.is_null() {
                DestroyWindow(self.hwnd);
            }
            if !self.font.is_null() {
                DeleteObject(self.font);
            }
            UnregisterClassW(self.class.as_ptr(), GetModuleHandleW(null()));
        }
    }
}

pub fn run(
    inbox: FrameInbox,
    choices: ReplySender<CandidateChoice>,
    close_label: &str,
) -> Result<(), String> {
    let _dpi = DpiContext::enter()?;
    let mut popup = Popup::new(close_label)?;
    let mut view = View::default();
    let event = inbox.event().as_raw_handle();
    loop {
        // SAFETY: 消息和有界邮箱共用事件等待，空闲不轮询；Inbox 持有事件。
        if unsafe {
            MsgWaitForMultipleObjectsEx(1, &event, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        } == WAIT_FAILED
        {
            return Err(error());
        }
        loop {
            match inbox.try_receive() {
                Ok(frame) => {
                    let serial = view.update(frame)?;
                    popup.display(view.frame.as_ref(), serial, true)?;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        let mut message = MSG::default();
        for _ in 0..64 {
            // SAFETY: 读取当前线程自己的消息；限制批量避免消息洪泛饿死快照更新。
            if unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } == 0 {
                break;
            }
            match message.message {
                CHOOSE if message.hwnd == popup.hwnd => {
                    if let Ok(index) = u8::try_from(message.lParam)
                        && let Some(choice) = view.choose(message.wParam as u32, index)
                    {
                        choices
                            .send(choice)
                            .map_err(|e| format!("候选点击未送达：{e}"))?;
                        popup.hide();
                    }
                }
                CLOSE if message.hwnd == popup.hwnd => return Ok(()),
                WM_QUIT => return Err("候选窗消息循环终止".into()),
                _ => {
                    // SAFETY: 未接管的原生控件消息按系统流程处理。
                    unsafe {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_popup_stamps_real_button_notifications_without_activation() {
        let _dpi = DpiContext::enter().unwrap();
        let mut popup = Popup::new("关闭预览").unwrap();
        let mut view = View {
            serial: u32::MAX - 2,
            ..Default::default()
        };
        let serial = view.update(Some(super::super::tests::frame())).unwrap();
        popup.display(view.frame.as_ref(), serial, false).unwrap();
        // SAFETY: 只检查隐藏测试窗，不向桌面注入鼠标或键盘输入。
        unsafe {
            assert_eq!(IsWindowVisible(popup.hwnd), 0);
            assert_ne!(
                GetWindowLongPtrW(popup.hwnd, GWL_EXSTYLE) as usize & WS_EX_NOACTIVATE as usize,
                0
            );
            SendMessageW(popup.hwnd, WM_COMMAND, 1, popup.controls[1] as isize);
        }
        let mut message = MSG::default();
        assert_ne!(
            // SAFETY: 读取本测试窗口刚生成的点击通知。
            unsafe { PeekMessageW(&mut message, popup.hwnd, CHOOSE, CHOOSE, PM_REMOVE) },
            0
        );
        assert_eq!(message.wParam, serial as usize);
        view.update(Some(super::super::tests::frame())).unwrap();
        assert!(
            view.choose(message.wParam as u32, message.lParam as u8)
                .is_none()
        );
    }
}
