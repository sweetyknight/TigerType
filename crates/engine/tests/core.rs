//! 合成词表覆盖状态机与学习契约，不读取真实资源、文件或个人输入。
use tigertype_contracts::{InputMode, Key, Options, OutputForm};
use tigertype_engine::{Data, Detail, Dictionary, Entry, Mapping, Pronunciation, Session};

fn data() -> Data {
    Data {
        entries: vec![
            Entry {
                character: '的',
                code: "u".into(),
                weight: 100,
            },
            Entry {
                character: '的',
                code: "unid".into(),
                weight: 1,
            },
            Entry {
                character: '发',
                code: "srid".into(),
                weight: 100,
            },
            Entry {
                character: '發',
                code: "hbrs".into(),
                weight: 50,
            },
            Entry {
                character: '髮',
                code: "peni".into(),
                weight: 40,
            },
            Entry {
                character: '行',
                code: "px".into(),
                weight: 30,
            },
            Entry {
                character: '女',
                code: "bn".into(),
                weight: 20,
            },
            Entry {
                character: '𠀀',
                code: "fgfa".into(),
                weight: 0,
            },
        ],
        common: vec!['的', '发', '女', '行'],
        pinyin: vec![
            Pronunciation {
                character: '行',
                pinyin: "xing".into(),
                weight: 8,
            },
            Pronunciation {
                character: '行',
                pinyin: "hang".into(),
                weight: 5,
            },
            Pronunciation {
                character: '女',
                pinyin: "nv".into(),
                weight: 6,
            },
            Pronunciation {
                character: '𠀀',
                pinyin: "he".into(),
                weight: 1,
            },
        ],
        details: vec![
            Detail {
                character: '的',
                full_codes: vec!["unid".into()],
                decomposition: Some("白勹丶".into()),
                readings: vec!["de".into()],
            },
            Detail {
                character: '發',
                full_codes: vec!["hbrs".into()],
                decomposition: Some("癶弓殳".into()),
                readings: vec!["fā".into()],
            },
        ],
        simplified: vec![Mapping {
            character: '發',
            variants: vec!['发'],
        }],
        traditional: vec![Mapping {
            character: '发',
            variants: vec!['發', '髮', '發'],
        }],
    }
}

#[test]
fn punctuation_pairs_and_ascii_punctuation_do_not_change_candidate_selection() {
    let dictionary = Dictionary::new(data()).unwrap();
    let mut session = Session::new(&dictionary, Options::default());
    assert_eq!(
        session
            .process(Key::Character('"'))
            .unwrap()
            .commit
            .as_deref(),
        Some("“")
    );
    type_code(&mut session, "u");
    assert_eq!(
        session
            .process(Key::Character('"'))
            .unwrap()
            .commit
            .as_deref(),
        Some("的”")
    );
    assert_eq!(
        session
            .process(Key::Character('\''))
            .unwrap()
            .commit
            .as_deref(),
        Some("‘")
    );
    type_code(&mut session, "u");
    assert!(
        session
            .process(Key::Character('\''))
            .unwrap()
            .commit
            .is_none()
    );
    session.clear().unwrap();
    assert_eq!(
        session
            .process(Key::Character('\''))
            .unwrap()
            .commit
            .as_deref(),
        Some("’")
    );
    session.process(Key::TogglePunctuation).unwrap();
    assert!(!session.process(Key::Character(',')).unwrap().handled);
    type_code(&mut session, "u");
    assert_eq!(
        session
            .process(Key::Character(','))
            .unwrap()
            .commit
            .as_deref(),
        Some("的,")
    );
}
fn type_code(session: &mut Session<'_>, code: &str) -> Vec<String> {
    code.chars()
        .filter_map(|c| session.process(Key::Character(c)).unwrap().commit)
        .collect()
}
#[test]
fn tsf_local_key_judgment_matches_engine_handling_in_every_reachable_state() {
    // TSF 在 OnTestKeyDown 只凭上次快照判定；任何偏差都会吞掉应用按键或漏掉输入。
    let dictionary = Dictionary::new(data()).unwrap();
    let letters = |code: &str| code.chars().map(Key::Character).collect::<Vec<_>>();
    let mut prefixes = vec![
        vec![],
        letters("u"),
        letters("unid"),
        letters("qqqq"),
        letters("`"),
        letters("`xing"),
        vec![Key::TogglePunctuation],
        [vec![Key::TogglePunctuation], letters("u")].concat(),
        vec![Key::ToggleAscii],
        vec![Key::ToggleAscii, Key::TogglePunctuation],
        [letters("u"), vec![Key::ToggleAscii, Key::ToggleAscii]].concat(),
        vec![Key::ToggleFullShape],
        [vec![Key::ToggleFullShape], letters("u")].concat(),
        vec![Key::ToggleFullShape, Key::TogglePunctuation],
        vec![Key::ToggleAscii, Key::ToggleFullShape],
    ];
    prefixes.push([letters("u"), vec![Key::Space]].concat());
    let mut keys = vec![
        Key::Space,
        Key::Enter,
        Key::Backspace,
        Key::Escape,
        Key::Tab,
        Key::PageUp,
        Key::PageDown,
        Key::Previous,
        Key::Next,
        Key::ToggleAscii,
        Key::ToggleLearning,
        Key::TogglePunctuation,
        Key::ToggleFullShape,
    ];
    keys.extend((0u8..=0x7f).map(|b| Key::Character(b as char)));
    keys.extend(['ü', 'Ü', '中', '１', '\u{3000}', '\u{85}'].map(Key::Character));
    for options in [
        Options::default(),
        Options {
            auto_commit_four: true,
            common_only: false,
            show_learning: true,
            output_form: OutputForm::Traditional,
            ..Default::default()
        },
    ] {
        for prefix in &prefixes {
            for key in &keys {
                let mut session = Session::new(&dictionary, options);
                for step in prefix {
                    session.process(*step).unwrap();
                }
                let predicted = session.snapshot().consumes(*key);
                let handled = session.process(*key).unwrap().handled;
                assert_eq!(predicted, handled, "前缀 {prefix:?} 按键 {key:?}");
            }
        }
    }
}
#[test]
fn full_shape_commits_full_width_forms_only_for_keys_that_would_pass_through() {
    let dictionary = Dictionary::new(data()).unwrap();
    let mut session = Session::new(&dictionary, Options::default());
    let commit = |session: &mut Session<'_>, key| {
        let outcome = session.process(key).unwrap();
        (outcome.handled, outcome.commit)
    };
    // 半角：数字、大写字母、空格照常放行给宿主。
    assert_eq!(commit(&mut session, Key::Character('1')), (false, None));
    assert!(!session.snapshot().full_shape);
    session.process(Key::ToggleFullShape).unwrap();
    assert!(session.snapshot().full_shape);
    for (key, wide) in [
        (Key::Character('1'), "１"),
        (Key::Character('A'), "Ａ"),
        (Key::Character('#'), "＃"),
        (Key::Character(','), "，"),
        (Key::Space, "\u{3000}"),
    ] {
        assert_eq!(
            commit(&mut session, key),
            (true, Some(wide.into())),
            "{key:?}"
        );
    }
    // 控制键不受影响；组合中的符号跟在首选后也转全角。
    assert_eq!(commit(&mut session, Key::Enter), (false, None));
    session.process(Key::TogglePunctuation).unwrap();
    type_code(&mut session, "u");
    assert_eq!(
        commit(&mut session, Key::Character('#')),
        (true, Some("的＃".into()))
    );
    // 英文模式下字母也转全角；再切回半角即放行。
    session.process(Key::ToggleAscii).unwrap();
    assert_eq!(
        commit(&mut session, Key::Character('a')),
        (true, Some("ａ".into()))
    );
    session.process(Key::ToggleFullShape).unwrap();
    assert_eq!(commit(&mut session, Key::Character('a')), (false, None));
}
#[test]
fn learning_comment_shows_decomposition_and_codes_only_when_enabled() {
    let dictionary = Dictionary::new(data()).unwrap();
    let comment = |show_learning| {
        let mut session = Session::new(
            &dictionary,
            Options {
                show_learning,
                ..Default::default()
            },
        );
        type_code(&mut session, "unid");
        session.snapshot().candidates[0].comment.clone()
    };
    // 开关关闭时不显示任何拆分与编码；打开后按小狼毫格式〔拆分 · 编码〕。
    assert_eq!(comment(false), "");
    assert_eq!(comment(true), "〔白勹丶 · u / unid〕");
}
#[test]
fn default_waits_four_then_next_letter_commits_once() {
    let dictionary = Dictionary::new(data()).unwrap();
    let mut session = Session::new(&dictionary, Options::default());
    assert!(type_code(&mut session, "unid").is_empty());
    assert_eq!(session.snapshot().commit_preview, "的");
    assert_eq!(type_code(&mut session, "u"), vec!["的"]);
    assert_eq!(session.snapshot().composition.text, "u");
    assert_eq!(
        session.process(Key::Space).unwrap().commit.as_deref(),
        Some("的")
    );
    assert!(!session.process(Key::Space).unwrap().handled);
}
#[test]
fn auto_commit_checks_converted_deduplicated_candidates_and_setting_changes_do_not_commit() {
    let dictionary = Dictionary::new(data()).unwrap();
    let options = Options {
        auto_commit_four: true,
        ..Default::default()
    };
    let mut session = Session::new(&dictionary, options);
    assert_eq!(type_code(&mut session, "unid"), vec!["的"]);
    session
        .configure(Options {
            output_form: OutputForm::Traditional,
            ..options
        })
        .unwrap();
    assert!(type_code(&mut session, "srid").is_empty());
    let state = session.snapshot();
    assert_eq!(
        state
            .candidates
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        vec!["發", "髮"]
    );
    assert_eq!(state.candidates[0].source, Some('发'));
    session.configure(options).unwrap();
    assert_eq!(session.snapshot().composition.text, "srid");
    assert_eq!(
        session.process(Key::Space).unwrap().commit.as_deref(),
        Some("发")
    );
}
#[test]
fn reverse_lookup_is_full_charset_and_reports_the_actual_character() {
    let dictionary = Dictionary::new(data()).unwrap();
    for (code, expected) in [
        ("`xing", "行"),
        ("`hang", "行"),
        ("`nü", "女"),
        ("`nv", "女"),
        ("`he", "𠀀"),
    ] {
        let mut session = Session::new(&dictionary, Options::default());
        assert!(type_code(&mut session, code).is_empty());
        let state = session.snapshot();
        assert_eq!(state.mode, InputMode::Pinyin);
        assert_eq!(state.candidates[0].text, expected);
        assert!(state.candidates[0].learning.is_some());
        assert_eq!(
            session.process(Key::Space).unwrap().commit.as_deref(),
            Some(expected)
        );
    }
    let mut session = Session::new(
        &dictionary,
        Options {
            output_form: OutputForm::Traditional,
            show_learning: true,
            ..Default::default()
        },
    );
    type_code(&mut session, "srid");
    let info = session.snapshot().candidates[0].learning.clone().unwrap();
    assert_eq!(info.codes, vec!["hbrs"]);
    assert!(!info.codes.contains(&"srid".into()));
}
#[test]
fn lookup_is_read_only_handles_missing_and_multiple_characters_and_bounds() {
    let dictionary = Dictionary::new(data()).unwrap();
    let mut session = Session::new(&dictionary, Options::default());
    type_code(&mut session, "u");
    let before = session.snapshot();
    assert_eq!(dictionary.lookup("NÜ", 0).unwrap().items[0].character, '女');
    let results = dictionary.lookup("的𠀀☃的", 0).unwrap();
    assert_eq!(results.items.len(), 3);
    assert!(results.items[2].learning.codes.is_empty());
    assert_eq!(results.items[0].learning.full_codes, vec!["unid"]);
    assert_eq!(session.snapshot(), before);
    assert!(dictionary.lookup(&"的".repeat(65), 0).is_err());
    assert!(dictionary.lookup("nihaoma", 0).is_err());
    assert!(dictionary.lookup("han", usize::MAX).is_err());
}
#[test]
fn empty_code_never_commits_and_full_empty_code_is_replaced_by_the_next_key() {
    let dictionary = Dictionary::new(data()).unwrap();
    let mut session = Session::new(&dictionary, Options::default());
    type_code(&mut session, "zzzz");
    // 满码空码时后码顶上：清掉无效编码、不上屏，本键开始新编码。
    let extra = session.process(Key::Character('u')).unwrap();
    assert!(extra.handled && extra.commit.is_none());
    assert_eq!(extra.state.composition.text, "u");
    assert!(!extra.state.candidates.is_empty());
    type_code(&mut session, "zzz");
    let space = session.process(Key::Space).unwrap();
    assert!(space.handled && space.commit.is_none());
    assert_eq!(space.state.composition.text, "uzzz");
    let digit = session.process(Key::Character('1')).unwrap();
    assert!(digit.handled && digit.commit.is_none());
    session.process(Key::Backspace).unwrap();
    assert_eq!(session.snapshot().composition.text, "uzz");
    session.process(Key::Tab).unwrap();
    assert!(!session.snapshot().composing);
    type_code(&mut session, "u");
    let toggle = session.process(Key::ToggleAscii).unwrap();
    assert_eq!(toggle.commit.as_deref(), Some("u"));
    assert!(!session.process(Key::Character('a')).unwrap().handled);
}
#[test]
fn pagination_selection_and_stale_snapshots_are_checked() {
    let mut data = data();
    for index in 0..21 {
        let character = char::from_u32(0x4e10 + index).unwrap();
        data.entries.push(Entry {
            character,
            code: "a".into(),
            weight: 100 - index as u64,
        });
        data.common.push(character);
    }
    let dictionary = Dictionary::new(data).unwrap();
    let mut session = Session::new(&dictionary, Options::default());
    type_code(&mut session, "a");
    let old = session.snapshot();
    // 每页标号都从阿拉伯数字 1 起，与数字键选重对应。
    let labels: Vec<_> = old.candidates.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["1", "2", "3", "4", "5", "6", "7", "8", "9"]);
    session.process(Key::PageDown).unwrap();
    assert_eq!(session.snapshot().page_index, 1);
    assert_eq!(session.snapshot().candidates[0].label, "1");
    assert!(session.select(old.revision, 0).is_err());
    let state = session.snapshot();
    assert!(session.select(state.revision, 9).is_err());
    let selected = session.select(state.revision, 1).unwrap().commit.unwrap();
    assert_eq!(selected, char::from_u32(0x4e10 + 10).unwrap().to_string());
    assert!(session.select(state.revision, 1).is_err());
}
#[test]
fn malformed_data_is_rejected_before_sessions_and_initialization_is_repeatable() {
    let mut bad = data();
    bad.entries[0].code = "toolong".into();
    assert!(Dictionary::new(bad).is_err());
    let mut bad = data();
    bad.pinyin[0].pinyin = "xi ng".into();
    assert!(Dictionary::new(bad).is_err());
    let mut bad = data();
    bad.traditional[0].variants.clear();
    assert!(Dictionary::new(bad).is_err());
    for _ in 0..100 {
        assert!(Dictionary::new(data()).is_ok());
    }
}
#[test]
fn stable_weight_order_dedup_and_common_filter_do_not_enable_learning() {
    let mut data = data();
    data.entries.push(Entry {
        character: '甲',
        code: "u".into(),
        weight: 100,
    });
    data.entries.push(Entry {
        character: '的',
        code: "u".into(),
        weight: 90,
    });
    let dictionary = Dictionary::new(data).unwrap();
    let mut session = Session::new(&dictionary, Options::default());
    type_code(&mut session, "u");
    assert_eq!(session.snapshot().candidates.len(), 1);
    session
        .configure(Options {
            common_only: false,
            ..Default::default()
        })
        .unwrap();
    let state = session.snapshot();
    assert_eq!(
        state
            .candidates
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        vec!["的", "甲"]
    );
    assert!(state.candidates.iter().all(|c| c.learning.is_none()));
    session.process(Key::ToggleLearning).unwrap();
    assert!(session.snapshot().candidates[0].learning.is_some());
}
