//! 隐藏候选窗：不在桌面显示、不注入系统输入，鼠标消息直接发给本测试线程的窗口。
use super::*;
use tigertype_contracts::{Candidate, InputMode, Theme, without_blue};
use windows::{
    Win32::{
        System::{
            Com::SAFEARRAY,
            Ole::{SafeArrayDestroy, SafeArrayGetElement, SafeArrayGetUBound},
            Variant::{VARIANT, VT_BSTR},
        },
        UI::Accessibility::*,
    },
    core::{HRESULT, IUnknown, Interface},
};

fn state(count: usize, highlighted: usize) -> InputState {
    InputState {
        composing: true,
        candidates: (0..count)
            .map(|index| Candidate {
                text: format!("字{index}"),
                comment: format!("u{index}"),
                label: (index + 1).to_string(),
                ..Default::default()
            })
            .collect(),
        page_size: 9,
        highlighted: Some(highlighted),
        revision: 7,
        ..Default::default()
    }
}
const CARET: Rect = Rect {
    left: 100,
    top: 100,
    right: 110,
    bottom: 120,
};
fn center(rect: &RECT) -> LPARAM {
    LPARAM(((((rect.top + rect.bottom) / 2) << 16) | ((rect.left + rect.right) / 2)) as isize)
}

#[test]
fn destroyed_owner_invalidates_candidates_even_while_rust_references_survive() {
    // Win32 会随 owner 销毁候选窗，Rc 存活不代表 HWND 仍归本对象所有。
    let popup = Popup::new(|_, _| {}, true).unwrap();
    // SAFETY: 只创建隐藏的合成宿主，下面显式销毁，不操作用户窗口。
    let owner = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            None,
            WS_POPUP,
            100,
            100,
            200,
            100,
            None,
            None,
            None,
            None,
        )
    }
    .unwrap();
    let sample = state(2, 0);
    popup.show(&sample, CARET, owner).unwrap();
    let serial = popup.view.borrow().serial;
    let item = uia::item(&popup, 0).unwrap();
    let root = popup.root();
    // SAFETY: 只销毁本测试 owner；系统应一并销毁它拥有的候选窗。
    unsafe {
        DestroyWindow(owner).unwrap();
        assert!(!IsWindow(Some(popup.hwnd)).as_bool());
    }
    assert!(!popup.showing(serial), "销毁后旧展示不能仍可点击");
    assert!(popup.anchor().is_none());
    assert!(popup.selectable(serial, 0).is_none());
    assert!(name(&item).is_err(), "旧 UIA 元素不得继续报告候选");
    // SAFETY: 旧根提供者不得把已销毁或复用的句柄交给 UIA。
    assert!(unsafe { root.HostRawElementProvider() }.is_err());
    assert!(popup.show(&sample, CARET, HWND::default()).is_err());
    popup.hide();
}

#[test]
fn unchanged_snapshot_still_rebinds_a_different_owner() {
    // 对话框/二级输入窗口变化时，候选与坐标不变也不能沿用旧 owner。
    let popup = Popup::new(|_, _| {}, true).unwrap();
    let make = || {
        // SAFETY: 仅创建隐藏测试窗口，测试末尾统一销毁。
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                None,
                WS_POPUP,
                100,
                100,
                200,
                100,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap()
    };
    let (first, second) = (make(), make());
    let sample = state(2, 0);
    popup.show(&sample, CARET, first).unwrap();
    let serial = popup.view.borrow().serial;
    popup.show(&sample, CARET, second).unwrap();
    // SAFETY: 只读取/销毁本测试窗口，owner 更换不作废同一快照。
    unsafe {
        assert_eq!(GetWindow(popup.hwnd, GW_OWNER).unwrap(), second);
        assert_eq!(popup.view.borrow().serial, serial);
        DestroyWindow(first).unwrap();
        assert!(IsWindow(Some(popup.hwnd)).as_bool());
        popup.hide();
        DestroyWindow(second).unwrap();
    }
}

#[test]
fn system_hidden_or_covered_popup_recovers_without_stealing_focus() {
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, HDESK, SetThreadDesktop,
    };
    // 真正显示与遮挡只在私有桌面的新线程上测试，不切换桌面、不注入用户输入。
    struct Desktop(HDESK);
    impl Drop for Desktop {
        fn drop(&mut self) {
            // SAFETY: 测试线程已经退出，只释放本测试创建的桌面。
            unsafe { CloseDesktop(self.0) }.unwrap();
        }
    }
    let name: Vec<u16> = format!("TigerType-CandidateTest-{}", std::process::id())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: 只创建未切换的私有测试桌面，名称包含独立测试进程号。
    let desktop = Desktop(
        unsafe {
            CreateDesktopW(
                PCWSTR(name.as_ptr()),
                None,
                None,
                DESKTOP_CONTROL_FLAGS(0),
                0x1000_0000,
                None,
            )
        }
        .unwrap(),
    );
    let handle = desktop.0.0 as usize;
    let result = std::thread::spawn(move || {
        // SAFETY: 新线程尚未创建任何窗口；桌面句柄由父线程持有到 join 后。
        unsafe { SetThreadDesktop(HDESK(handle as *mut _)) }.unwrap();
        let make = |owner| {
            // SAFETY: 只创建私有桌面内的测试窗口，下面配对销毁。
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    None,
                    WS_POPUP,
                    100,
                    100,
                    300,
                    200,
                    owner,
                    None,
                    None,
                    None,
                )
            }
            .unwrap()
        };
        let owner = make(None);
        // SAFETY: 只显示私有桌面的窗口，并在本线程建立焦点，用户桌面不受影响。
        unsafe {
            SetWindowPos(
                owner,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
            .unwrap();
            // 返回的是原窗口，首次为 NULL 不表示失败；用实际焦点核对。
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetActiveWindow(owner);
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(owner));
            assert_eq!(GetFocus(), owner);
        }
        let popup = Popup::new(|_, _| {}, false).unwrap();
        let watched = Rc::downgrade(&popup);
        *popup.before_notify.borrow_mut() = Some(Box::new(move |event| {
            if matches!(event, EVENT_OBJECT_IME_SHOW | EVENT_OBJECT_IME_CHANGE) {
                let popup = watched.upgrade().unwrap();
                // SAFETY: 仅检查私有桌面测试窗口，发送通知时必须已无待绘制区域。
                let dirty = unsafe { GetUpdateRect(popup.hwnd, None, false) }.as_bool();
                assert!(!dirty, "外部通知不能先于首帧绘制");
            }
        }));
        let mut sample = state(2, 0);
        popup.show(&sample, CARET, owner).unwrap();
        let serial = popup.view.borrow().serial;
        assert_eq!(popup.selectable(serial, 0), Some(sample.revision));
        for updated in [false, true] {
            // SAFETY: 模拟系统/宿主绕过 Popup::hide 隐藏窗口，不改本地快照。
            unsafe {
                SetWindowPos(
                    popup.hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
                .unwrap();
                assert!(!IsWindowVisible(popup.hwnd).as_bool());
            }
            if updated {
                sample.revision += 1;
            }
            popup.show(&sample, CARET, owner).unwrap();
            // SAFETY: 仅查询私有桌面中的测试窗口，重新显示不得抢走宿主焦点。
            unsafe {
                assert!(
                    IsWindowVisible(popup.hwnd).as_bool(),
                    "系统隐藏后必须恢复：updated={updated}"
                );
                assert_eq!(GetFocus(), owner);
            }
            if !updated {
                assert_eq!(popup.view.borrow().serial, serial);
                assert!(popup.view.borrow().pending, "仅恢复可见性不作废在途点击");
            }
        }
        // 同一宿主的后开浮层可能覆盖候选；同快照刷新也要恢复层级，不提升为全局 topmost。
        let covering = make(Some(owner));
        // SAFETY: 同上，只在私有桌面内模拟后开窗口。
        unsafe {
            SetWindowPos(
                covering,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
            .unwrap();
        }
        let above = |higher, lower| {
            let mut window = higher;
            for _ in 0..32 {
                // SAFETY: 有界只读枚举本私有桌面的 z-order。
                window = unsafe { GetWindow(window, GW_HWNDNEXT) }.unwrap_or_default();
                if window == lower {
                    return true;
                }
                if window.is_invalid() {
                    break;
                }
            }
            false
        };
        assert!(above(covering, popup.hwnd));
        popup.show(&sample, CARET, owner).unwrap();
        assert!(
            above(popup.hwnd, covering),
            "刷新候选必须高于同宿主的遮挡浮层"
        );
        // SAFETY: 检查非全局置顶、不抢焦点；最后只销毁测试窗口。
        unsafe {
            assert_eq!(
                GetWindowLongW(popup.hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0,
                0
            );
            assert_eq!(GetFocus(), owner);
            popup.hide();
            DestroyWindow(covering).unwrap();
            DestroyWindow(owner).unwrap();
        }
    })
    .join();
    drop(desktop);
    result.unwrap();
}
/// 主显示器工作区；用真实边界验证翻转与夹取，不依赖本机分辨率。
fn primary_work() -> Rect {
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: 输出结构大小已填写；(0,0) 总在主显示器上。
    unsafe {
        let primary = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
        assert!(GetMonitorInfoW(primary, &mut monitor).as_bool());
    }
    portable(monitor.rcWork)
}

#[test]
fn visible_update_never_passes_through_the_raw_composition_position() {
    // 输入框贴屏幕底部时候选窗翻到上方；先移到组合下方再翻回，屏幕上就是逐键上下跳。
    // 候选窗按逐显示器 DPI 创建；本线程取同一上下文，核对的才是未被虚拟化的物理坐标。
    // SAFETY: 只改本测试线程的 DPI 上下文，线程随测试结束。
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let work = primary_work();
    let popup = Popup::new(|_, _| {}, true).unwrap();
    let caret = |left| Rect {
        left,
        top: work.bottom - 30,
        right: left + 10,
        bottom: work.bottom - 10,
    };
    popup
        .show(&state(3, 0), caret(200), HWND::default())
        .unwrap();
    popup.moves.borrow_mut().clear();
    let mut next = state(2, 0);
    next.revision += 1;
    popup.show(&next, caret(230), HWND::default()).unwrap();
    let moves = popup.moves.borrow().clone();
    assert!(!moves.is_empty(), "新位置必须真正移动");
    let height = popup.view.borrow().layout.size.cy;
    for (x, y) in moves {
        assert_eq!(x, 230);
        assert_eq!(y, work.bottom - 30 - height, "不得经过未夹取的组合下方位置");
    }
    // 右边缘同理：不能先越出屏幕再夹回。
    popup.moves.borrow_mut().clear();
    let edge = Rect {
        left: work.right - 5,
        top: 300,
        right: work.right,
        bottom: 320,
    };
    next.revision += 1;
    popup.show(&next, edge, HWND::default()).unwrap();
    let width = popup.view.borrow().layout.size.cx;
    for (x, _) in popup.moves.borrow().iter() {
        assert_eq!(*x, work.right - width, "不得越出工作区再夹回");
    }
    popup.hide();
}

#[test]
fn layout_growth_that_keeps_the_placement_does_not_touch_the_window() {
    // 组合变长只改矩形右缘，候选窗位置不变：宿主每次布局通知都重摆重画只是浪费宿主 UI 线程。
    let popup = Popup::new(|_, _| {}, true).unwrap();
    let sample = state(3, 0);
    popup.show(&sample, CARET, HWND::default()).unwrap();
    let serial = popup.view.borrow().serial;
    let notified = Rc::new(Cell::new(0));
    let counter = notified.clone();
    *popup.before_notify.borrow_mut() = Some(Box::new(move |_| counter.set(counter.get() + 1)));
    let wider = Rect {
        right: CARET.right + 40,
        ..CARET
    };
    popup.show(&sample, wider, HWND::default()).unwrap();
    assert_eq!(notified.get(), 0, "位置未变不重画、不发外部通知");
    assert_eq!(popup.anchor(), Some(wider), "退路位置仍要跟上最新组合矩形");
    assert_eq!(popup.view.borrow().serial, serial);
    popup.hide();
}

/// 只取出本窗口的点击通知并按系统流程派发，不处理其他消息。
fn pump(popup: &Popup) {
    let mut message = MSG::default();
    // SAFETY: 读取并派发本测试线程、本窗口的私有消息。
    unsafe {
        while PeekMessageW(&mut message, Some(popup.hwnd), CLICK, CLICK, PM_REMOVE).as_bool() {
            DispatchMessageW(&message);
        }
    }
}
fn press(popup: &Popup, down: &RECT, up: &RECT) {
    // SAFETY: 直接发给本测试窗口，不移动真实鼠标。
    unsafe {
        SendMessageW(popup.hwnd, WM_LBUTTONDOWN, None, Some(center(down)));
        SendMessageW(popup.hwnd, WM_LBUTTONUP, None, Some(center(up)));
    }
    pump(popup);
}

/// 在内存位图上调用真实绘制并读像素，不显示窗口、不碰桌面。
fn render(popup: &Popup, size: SIZE) -> impl Fn(i32, i32) -> COLORREF {
    // SAFETY: 内存 DC 与位图只在本测试内使用，读完像素后由闭包持有直到测试结束（测试进程退出回收）。
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, size.cx, size.cy);
        ReleaseDC(None, screen);
        SelectObject(dc, bitmap.into());
        let client = RECT {
            right: size.cx,
            bottom: size.cy,
            ..Default::default()
        };
        popup.draw(dc, &client);
        move |x, y| GetPixel(dc, x, y)
    }
}

#[test]
fn rendering_uses_theme_colors_in_gdi_byte_order_and_shows_footer_only_when_needed() {
    // 主题为 0xRRGGBB，GDI 为 0x00BBGGRR；字节序错会把琥珀画成蓝色。
    assert_eq!(rgb(0xB45309), COLORREF(0x0953B4));
    for theme in Theme::ALL {
        let palette = Options {
            theme,
            ..Default::default()
        }
        .palette();
        let c = themed(palette);
        for color in [c.window, c.highlight, c.text, c.chosen, c.border] {
            let [red, green, blue, _] = color.0.to_le_bytes();
            let value = u32::from_be_bytes([0, red, green, blue]);
            assert!(without_blue(value), "{theme:?} {color:?}");
        }
    }
    let popup = Popup::new(|_, _| {}, true).unwrap();
    let mut shown = state(3, 1);
    shown.options.theme = Theme::Bamboo;
    shown.is_last_page = true;
    popup.show(&shown, CARET, HWND::default()).unwrap();
    let layout = popup.view.borrow().layout.clone();
    assert!(layout.footer.is_none());
    // 无提示行：窗口高度恰为候选行加上下内边距。
    assert_eq!(layout.size.cy, layout.rows[2].bottom + layout.rows[0].top);
    assert!(layout.text_x < layout.comment_x && layout.comment_x < layout.size.cx);
    let palette = shown.options.palette();
    // 用户开启高对比度时改用系统色，不核对主题像素。
    if colors(palette) == themed(palette) {
        let c = themed(palette);
        let pixel = render(&popup, layout.size);
        let (normal, chosen) = (layout.rows[0], layout.rows[1]);
        assert_eq!(pixel(normal.right - 3, normal.top + 2), c.window);
        assert_eq!(pixel(chosen.left + 10, chosen.top + 2), c.highlight);
        // 高亮行只有圆角底色，左缘不再画强调指示条。
        assert_eq!(
            pixel(chosen.left + 1, (chosen.top + chosen.bottom) / 2),
            c.highlight
        );
        // 玫红主题高亮行无底色、无指示条，只把字染成玫红。
        shown.options.theme = Theme::Rose;
        popup.show(&shown, CARET, HWND::default()).unwrap();
        let layout = popup.view.borrow().layout.clone();
        let c = themed(shown.options.palette());
        let pixel = render(&popup, layout.size);
        let chosen = layout.rows[1];
        let middle = (chosen.top + chosen.bottom) / 2;
        assert_eq!(pixel(chosen.left + 1, middle), c.window);
        assert_eq!(pixel(chosen.left + 10, chosen.top + 2), c.window);
        let reddish = (layout.text_x..chosen.right).any(|x| {
            (chosen.top..chosen.bottom).any(|y| {
                let [red, green, ..] = pixel(x, y).0.to_le_bytes();
                red > green.saturating_add(96)
            })
        });
        assert!(reddish, "高亮候选字应为玫红");
        shown.options.theme = Theme::Bamboo;
    }
    // 还有下一页或拼音反查时才出现页脚。
    shown.is_last_page = false;
    popup.show(&shown, CARET, HWND::default()).unwrap();
    assert!(popup.view.borrow().layout.footer.is_some());
    assert!(footer(&shown).is_some_and(|footer| footer.pager.is_some()));
    // 拼音反查只有一页时页脚只留标签，不画翻页指示。
    shown.is_last_page = true;
    shown.mode = InputMode::Pinyin;
    popup.show(&shown, CARET, HWND::default()).unwrap();
    assert!(popup.view.borrow().layout.footer.is_some());
    assert!(footer(&shown).is_some_and(|footer| footer.pager.is_none()));
    // 候选文字来自后台进程：为空时不得把空 Vec 的悬空指针交给 DrawTextW 而让宿主崩溃。
    shown.candidates[0].text.clear();
    shown.candidates[0].label.clear();
    popup.show(&shown, CARET, HWND::default()).unwrap();
    let size = popup.view.borrow().layout.size;
    let _ = render(&popup, size);
}

/// 测试不依赖机器预装字体：本进程只私有注册一次仓库原件，随进程结束释放。
pub(crate) fn register_test_font() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let path: Vec<u16> = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../resources/fonts/LXGWWenKaiGBScreen.ttf")
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: 路径 NUL 结尾；注册只在本进程可见。
        let added = unsafe { AddFontResourceExW(PCWSTR(path.as_ptr()), FR_PRIVATE, None) };
        assert_ne!(added, 0);
    });
}

#[test]
fn learning_font_keeps_private_roots_when_candidate_font_changes() {
    register_test_font();
    let popup = Popup::new(|_, _| {}, true).unwrap();
    let mut shown = state(4, 0);
    shown.is_last_page = true;
    for (candidate, (word, parts)) in shown.candidates.iter_mut().zip([
        ("你", "亻\u{e4e4}小 · jx / jxyx"),
        ("这", "文辶 · vu / vuc"),
        ("题", "日\u{e46a}页 · ot / otw / otwy"),
        ("缺", "缶\u{e432}人 · an / anj / anjr"),
    ]) {
        candidate.text = word.into();
        candidate.comment = format!("〔{parts}〕");
    }
    let probe = wide("亻\u{e4e4}小文辶日\u{e46a}页缶\u{e432}人〔 · jxyx / 123〕");
    for face in CandidateFont::ALL {
        for dpi in [96, 144, 192] {
            popup.ensure_fonts(dpi, face).unwrap();
            let fonts = popup.fonts.get();
            let layout = popup.measure(&shown).unwrap();
            // SAFETY: 本地内存 DC 只读字体覆盖和排版；归还选中对象后删除 DC。
            unsafe {
                let dc = CreateCompatibleDC(None);
                assert!(!dc.is_invalid());
                let old = SelectObject(dc, fonts.comment.into());
                let mut glyphs = vec![0; probe.len()];
                assert_ne!(
                    GetGlyphIndicesW(
                        dc,
                        PCWSTR(probe.as_ptr()),
                        probe.len() as i32,
                        glyphs.as_mut_ptr(),
                        GGI_MARK_NONEXISTING_GLYPHS
                    ),
                    u32::MAX
                );
                assert!(
                    glyphs.iter().all(|g| *g != 0xffff && *g != 0),
                    "{face:?} / {dpi} DPI 拆分缺字"
                );
                assert!(face_is(dc, fonts.comment, LEARNING_FACES[0]));
                let mut widest = 0;
                for candidate in &shown.candidates {
                    let mut rect = RECT::default();
                    DrawTextW(
                        dc,
                        &mut wide(&candidate.comment),
                        &mut rect,
                        DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
                    );
                    assert_eq!(
                        popup.extent(dc, fonts.comment, &candidate.comment),
                        rect.right
                    );
                    widest = widest.max(rect.right);
                }
                assert_eq!(
                    layout.size.cx,
                    (layout.comment_x + widest + popup.scale(ROW_RIGHT + PAD))
                        .clamp(popup.scale(MIN_WIDTH), popup.scale(MAX_WIDTH))
                );
                assert_eq!(popup.extent(dc, fonts.comment, ""), 0);
                SelectObject(dc, old);
                let _ = DeleteDC(dc);
            }
        }
    }
    popup.show(&shown, CARET, HWND::default()).unwrap();
    let _ = render(&popup, popup.view.borrow().layout.size);
}

#[test]
fn missing_font_falls_back_to_the_next_face_in_priority_order() {
    // SAFETY: 屏幕 DC 只用于核对字体名，结束前归还；字体用后删除。
    unsafe {
        let dc = GetDC(None);
        let font = create_font(dc, &["TigerType 不存在的字体", UI_FACE], 15, 96, 400).unwrap();
        assert!(face_is(dc, font, UI_FACE));
        assert!(!face_is(dc, font, "TigerType 不存在的字体"));
        let _ = DeleteObject(font.into());
        ReleaseDC(None, dc);
    }
}

#[test]
fn hidden_popup_lays_out_rows_and_turns_matching_press_release_into_one_stamped_click() {
    let clicks = Rc::new(RefCell::new(Vec::new()));
    let sink = clicks.clone();
    let popup = Popup::new(
        move |serial, index| sink.borrow_mut().push((serial, index)),
        true,
    )
    .unwrap();
    popup.show(&state(3, 1), CARET, HWND::default()).unwrap();
    // SAFETY: 只读取本测试窗口属性。
    unsafe {
        assert!(!IsWindowVisible(popup.hwnd).as_bool());
        assert_ne!(
            GetWindowLongW(popup.hwnd, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE.0,
            0
        );
        assert_eq!(
            SendMessageW(popup.hwnd, WM_MOUSEACTIVATE, None, None),
            LRESULT(MA_NOACTIVATE as isize)
        );
    }
    let (serial, rows) = {
        let view = popup.view.borrow();
        (view.serial, view.layout.rows.clone())
    };
    assert_eq!(rows.len(), 3);
    assert!(
        rows.windows(2)
            .all(|pair| pair[0].bottom == pair[1].top && pair[0].top < pair[0].bottom)
    );
    let mut window = RECT::default();
    // SAFETY: 只读本窗口矩形。
    unsafe { GetWindowRect(popup.hwnd, &mut window).unwrap() };
    // 贴着组合上下沿放置（是否翻转取决于测试机工作区），宽度容纳全部行。
    assert!(window.top == CARET.bottom || window.bottom == CARET.top);
    assert!(
        rows.iter()
            .all(|row| row.right < window.right - window.left)
    );
    press(&popup, &rows[1], &rows[1]);
    // 按下与抬起不在同一行不算点击。
    press(&popup, &rows[0], &rows[2]);
    assert_eq!(*clicks.borrow(), [(serial, 1)]);
    // SAFETY: 本线程不再持有捕获。
    assert_ne!(unsafe { GetCapture() }, popup.hwnd);

    // 同一展示只受理一个点击；越界序号与旧展示编号都不受理。
    assert_eq!(popup.selectable(serial, 1), Some(7));
    assert!(popup.selectable(serial, 0).is_none());
    popup.unblock(serial);
    assert!(popup.selectable(serial, 3).is_none());
    assert!(popup.showing(serial));
    popup.show(&state(2, 0), CARET, HWND::default()).unwrap();
    assert!(!popup.showing(serial) && popup.selectable(serial, 0).is_none());
    popup.hide();
    let view = popup.view.borrow();
    assert!(view.state.is_none() && view.layout.rows.is_empty() && view.serial == serial + 2);
    // 隐藏后必须释放可合成的窗口面积，不能只清空候选内容而保留旧圆角边框的矩形。
    let mut hidden = RECT::default();
    // SAFETY: 只读本测试创建并仍持有的窗口，不操作用户宿主。
    unsafe {
        GetWindowRect(popup.hwnd, &mut hidden).unwrap();
        assert!(!IsWindowVisible(popup.hwnd).as_bool());
    }
    assert_eq!(
        (hidden.right - hidden.left, hidden.bottom - hidden.top),
        (0, 0)
    );
    drop(view);
    // 同一窗口和字体缓存继续复用；下一次组合须重新计算正常尺寸。
    popup.show(&state(2, 0), CARET, HWND::default()).unwrap();
    assert_eq!(popup.view.borrow().layout.rows.len(), 2);
    let mut restored = RECT::default();
    // SAFETY: 同上，只读隐藏测试窗口的重新布局结果。
    unsafe { GetWindowRect(popup.hwnd, &mut restored).unwrap() };
    assert!(restored.right > restored.left && restored.bottom > restored.top);
}

#[test]
fn popup_refresh_pressure_stays_within_input_budget() {
    let popup = Popup::new(|_, _| {}, true).unwrap();
    let mut sample = state(9, 0);
    sample.options.show_learning = true;
    let started = std::time::Instant::now();
    for revision in 0..1_000 {
        sample.revision = revision + 1;
        sample.highlighted = Some(revision as usize % sample.candidates.len());
        popup.show(&sample, CARET, HWND::default()).unwrap();
    }
    let elapsed = started.elapsed();
    eprintln!(
        "candidate popup pressure: n=1000 total={elapsed:?} per={:?}",
        elapsed / 1_000
    );
    assert!(
        elapsed <= std::time::Duration::from_secs(1),
        "候选窗刷新过慢: {elapsed:?}"
    );
}

fn string(value: &VARIANT) -> String {
    // SAFETY: 先核对类型标记为 VT_BSTR，再借用联合体中的字符串；VARIANT 仍拥有它。
    unsafe {
        let inner = &value.Anonymous.Anonymous;
        assert_eq!(inner.vt, VT_BSTR);
        inner.Anonymous.bstrVal.to_string()
    }
}
fn name(provider: &IRawElementProviderSimple) -> Result<String> {
    // SAFETY: 本线程提供者的同步属性查询。
    let value = unsafe { provider.GetPropertyValue(UIA_NamePropertyId)? };
    Ok(string(&value))
}
fn simple(fragment: &IRawElementProviderFragment) -> IRawElementProviderSimple {
    fragment.cast().unwrap()
}
fn selected(provider: &IRawElementProviderSimple) -> bool {
    // SAFETY: 本线程提供者的同步模式查询。
    unsafe {
        provider
            .GetPatternProvider(UIA_SelectionItemPatternId)
            .unwrap()
            .cast::<ISelectionItemProvider>()
            .unwrap()
            .IsSelected()
            .unwrap()
            .as_bool()
    }
}
fn runtime_id(fragment: &IRawElementProviderFragment) -> Vec<i32> {
    // SAFETY: 读取并销毁提供者交出的一维 VT_I4 数组。
    unsafe {
        let array: *mut SAFEARRAY = fragment.GetRuntimeId().unwrap();
        let last = SafeArrayGetUBound(array, 1).unwrap();
        let parts = (0..=last)
            .map(|index| {
                let mut value = 0i32;
                SafeArrayGetElement(array, &index, (&mut value as *mut i32).cast()).unwrap();
                value
            })
            .collect();
        SafeArrayDestroy(array).unwrap();
        parts
    }
}

#[test]
fn uia_follows_ime_conventions_and_old_items_expire_with_the_display() {
    let popup = Popup::new(|_, _| {}, true).unwrap();
    popup.show(&state(3, 1), CARET, HWND::default()).unwrap();
    let serial = popup.view.borrow().serial;
    let root = popup.root();
    // SAFETY: 以下均为本线程合成窗口提供者的同步 COM 调用。
    unsafe {
        assert_ne!(
            root.ProviderOptions().unwrap().0 & ProviderOptions_UseComThreading.0,
            0
        );
        let id = root.GetPropertyValue(UIA_AutomationIdPropertyId).unwrap();
        assert_eq!(string(&id), "IME_Candidate_Window");
        let kind = root.GetPropertyValue(UIA_ControlTypePropertyId).unwrap();
        assert_eq!(i32::try_from(&kind).unwrap(), UIA_ListControlTypeId.0);
        let tree: IRawElementProviderFragment = root.cast().unwrap();
        let first = tree.Navigate(NavigateDirection_FirstChild).unwrap();
        let second = first.Navigate(NavigateDirection_NextSibling).unwrap();
        let last = tree.Navigate(NavigateDirection_LastChild).unwrap();
        assert_eq!(
            [&first, &second, &last].map(|item| name(&simple(item)).unwrap()),
            ["字0", "字1", "字2"]
        );
        let help = simple(&first)
            .GetPropertyValue(UIA_HelpTextPropertyId)
            .unwrap();
        assert_eq!(string(&help), "u0");
        assert!(!selected(&simple(&first)) && selected(&simple(&second)));
        assert!(first.Navigate(NavigateDirection_PreviousSibling).is_err());
        assert!(last.Navigate(NavigateDirection_NextSibling).is_err());
        let parent = simple(&first.Navigate(NavigateDirection_Parent).unwrap());
        assert_eq!(
            parent.cast::<IUnknown>().unwrap(),
            root.cast::<IUnknown>().unwrap()
        );
        assert_eq!(
            runtime_id(&second),
            [UiaAppendRuntimeId as i32, serial as i32, 1]
        );
        // 选字只能经编辑会话，UIA 选择被拒绝。
        assert!(
            simple(&second)
                .GetPatternProvider(UIA_SelectionItemPatternId)
                .unwrap()
                .cast::<ISelectionItemProvider>()
                .unwrap()
                .Select()
                .is_err()
        );
        let bounds = second.BoundingRectangle().unwrap();
        assert!(bounds.width > 0.0 && bounds.height > 0.0);
        let hit = root
            .cast::<IRawElementProviderFragmentRoot>()
            .unwrap()
            .ElementProviderFromPoint(
                bounds.left + bounds.width / 2.0,
                bounds.top + bounds.height / 2.0,
            )
            .unwrap();
        assert_eq!(name(&simple(&hit)).unwrap(), "字1");

        popup.show(&state(2, 0), CARET, HWND::default()).unwrap();
        let gone = HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32);
        assert_eq!(name(&simple(&second)).unwrap_err().code(), gone);
        assert_eq!(
            first
                .Navigate(NavigateDirection_NextSibling)
                .unwrap_err()
                .code(),
            gone
        );
        popup.hide();
        assert!(tree.Navigate(NavigateDirection_FirstChild).is_err());
    }
}
