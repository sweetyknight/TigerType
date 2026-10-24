//! 隐藏候选窗：不在桌面显示、不注入系统输入，鼠标消息直接发给本测试线程的窗口。
use super::*;
use tigertype_contracts::{Candidate, Theme, without_blue};
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
const CARET: RECT = RECT {
    left: 100,
    top: 100,
    right: 110,
    bottom: 120,
};
fn center(rect: &RECT) -> LPARAM {
    LPARAM(((((rect.top + rect.bottom) / 2) << 16) | ((rect.left + rect.right) / 2)) as isize)
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
        for color in [c.window, c.highlight, c.bar, c.text, c.chosen, c.border] {
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
        assert_eq!(
            pixel(chosen.left + 1, (chosen.top + chosen.bottom) / 2),
            c.bar
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
    shown.is_last_page = true;
    shown.mode = InputMode::Pinyin;
    popup.show(&shown, CARET, HWND::default()).unwrap();
    assert!(popup.view.borrow().layout.footer.is_some());
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
fn position_stays_inside_a_negative_coordinate_monitor_and_flips_above_the_bottom_edge() {
    let work = RECT {
        left: -1920,
        top: -100,
        right: 0,
        bottom: 980,
    };
    let caret = RECT {
        left: -10,
        top: 960,
        right: 0,
        bottom: 980,
    };
    let expected = RECT {
        left: -300,
        top: 560,
        right: 0,
        bottom: 960,
    };
    assert_eq!(position(caret, work, 300, 400), expected);
    assert_eq!(position(work, work, 4000, 4000), work);
    assert!(
        bounded(&CARET)
            && !bounded(&RECT {
                left: 1,
                ..RECT::default()
            })
    );
    assert!(!bounded(&RECT {
        right: 2_000_000,
        ..RECT::default()
    }));
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
