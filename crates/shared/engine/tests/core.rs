//! 合成词表覆盖状态机与学习契约，不读取真实资源、文件或个人输入。
use tigertype_contracts::{
    InputMode, InputState, Key, MAX_PINYIN_BYTES, Options, OutputForm, TypingMode,
};
use tigertype_engine::{
    Data, Detail, Dictionary, Entry, Mapping, Phrase, Pronunciation, Session, WordPinyin,
};

fn data() -> Data<'static> {
    Data {
        entries: vec![
            Entry {
                character: '的',
                code: "u",
                weight: 100,
            },
            Entry {
                character: '的',
                code: "unid",
                weight: 1,
            },
            Entry {
                character: '发',
                code: "srid",
                weight: 100,
            },
            Entry {
                character: '發',
                code: "hbrs",
                weight: 50,
            },
            Entry {
                character: '髮',
                code: "peni",
                weight: 40,
            },
            Entry {
                character: '行',
                code: "px",
                weight: 30,
            },
            Entry {
                character: '女',
                code: "bn",
                weight: 20,
            },
            Entry {
                character: '𠀀',
                code: "fgfa",
                weight: 0,
            },
        ],
        phrases: vec![Phrase {
            text: "我们",
            code: "tuja",
            weight: 10,
        }],
        common: vec!['的', '发', '女', '行'],
        pinyin: vec![
            Pronunciation {
                character: '行',
                pinyin: "xing",
                weight: 8,
            },
            Pronunciation {
                character: '行',
                pinyin: "hang",
                weight: 5,
            },
            Pronunciation {
                character: '女',
                pinyin: "nv",
                weight: 6,
            },
            Pronunciation {
                character: '𠀀',
                pinyin: "he",
                weight: 1,
            },
        ],
        details: vec![
            Detail {
                character: '的',
                full_codes: vec!["unid"],
                decomposition: Some("白勹丶"),
                readings: vec!["de"],
            },
            Detail {
                character: '發',
                full_codes: vec!["hbrs"],
                decomposition: Some("癶弓殳"),
                readings: vec!["fā"],
            },
        ],
        simplified: vec![mapping("發", &["发"])],
        traditional: vec![mapping("发", &["發", "髮", "發"])],
        ..Default::default()
    }
}
fn mapping<'a>(text: &'a str, variants: &[&'a str]) -> Mapping<'a> {
    Mapping {
        text,
        variants: variants.to_vec(),
    }
}

#[test]
fn word_mode_uses_tigress_codes_and_lookup_returns_phrase_text() {
    let dictionary = Dictionary::new(data()).unwrap();
    let mut session = Session::new(&dictionary, word_options());
    let outcome = ["tuja".chars().map(Key::Character).collect::<Vec<_>>()]
        .concat()
        .into_iter()
        .map(|key| session.process(key).unwrap())
        .last()
        .unwrap();
    assert_eq!(outcome.state.candidates[0].text, "我们");
    assert_eq!(dictionary.lookup("我们", 0).unwrap().items[0].text, "我们");
}

fn word_options() -> Options {
    Options {
        typing_mode: TypingMode::Word,
        common_only: false,
        ..Options::default()
    }
}
/// 权重取自官方：单字表与词版单字表故意不同，证明词模式只认词版权重。
fn word_data() -> Data<'static> {
    let entry = |character, code: &'static str, weight| Entry {
        character,
        code,
        weight,
    };
    let phrase = |text: &'static str, code: &'static str, weight| Phrase { text, code, weight };
    Data {
        entries: vec![entry('一', "fi", 4_346_343)],
        word_entries: vec![
            entry('的', "u", 10_359_470),
            entry('一', "fi", 256),
            entry('治', "kk", 500),
            entry('他', "je", 2_260_948),
            entry('我', "tupg", 256),
            entry('魑', "wr", 485),
        ],
        phrases: vec![
            phrase("工作", "u", 5000),
            phrase("工作", "ugjw", 36401),
            phrase("一般", "fi", 485),
            phrase("渐渐", "kk", 485),
            phrase("魑魅", "wr", 485),
            phrase("头发", "wmfy", 900),
            phrase("喉头发", "wmfy", 800),
            phrase("甲，乙", "wmfy", 600),
            phrase("甲乙", "wmfy", 500),
            phrase("乾隆", "wmfy", 450),
            phrase("甲乙", "wmfy", 400),
            phrase("表面", "wmfz", 2),
            phrase("錶面", "wmfz", 1),
        ],
        common: "的一治他我工作般渐头发喉甲乙".chars().collect(),
        simplified: vec![mapping("乾隆", &["乾隆"]), mapping("乾", &["干", "乾"])],
        traditional: vec![
            mapping("头发", &["頭髮"]),
            mapping("表面", &["表面", "錶面"]),
            mapping("发", &["發", "髮"]),
            mapping("头", &["頭"]),
            mapping("面", &["面", "麵"]),
        ],
        ..Default::default()
    }
}
fn type_code_state(session: &mut Session<'_>, code: &str) -> InputState {
    type_code(session, code);
    session.snapshot()
}
/// 新组合下该码的全部候选（跨页）。
fn texts(session: &mut Session<'_>, code: &str) -> Vec<String> {
    session.clear().unwrap();
    let mut state = type_code_state(session, code);
    let mut result = Vec::new();
    loop {
        result.extend(state.candidates.iter().map(|c| c.text.clone()));
        if state.is_last_page {
            return result;
        }
        state = session.process(Key::PageDown).unwrap().state;
    }
}

#[test]
fn word_mode_mixes_characters_and_words_by_official_weight() {
    let dictionary = Dictionary::new(word_data()).unwrap();
    let mut session = Session::new(&dictionary, word_options());
    assert_eq!(texts(&mut session, "u"), ["的", "工作"]);
    // 词版单字权重 256 低于简词 485；单字表的 4346343 会把“一”排回首选。
    assert_eq!(texts(&mut session, "fi"), ["一般", "一"]);
    assert_eq!(texts(&mut session, "kk"), ["治", "渐渐"]);
    // 同权重保持单字表在前，同 librime 的稳定排序。
    assert_eq!(texts(&mut session, "wr"), ["魑", "魑魅"]);
    assert_eq!(texts(&mut session, "ugjw"), ["工作"]);
    // 中间隔着其他词的同码同词也只出现一次。
    let bucket = texts(&mut session, "wmfy");
    assert_eq!(bucket.iter().filter(|text| *text == "甲乙").count(), 1);
    assert_eq!(texts(&mut session, "zz"), Vec::<String>::new());
}

#[test]
fn word_mode_commits_single_characters_like_official_word_edition() {
    let dictionary = Dictionary::new(word_data()).unwrap();
    let mut session = Session::new(&dictionary, word_options());
    type_code(&mut session, "u");
    assert_eq!(
        session.process(Key::Space).unwrap().commit.as_deref(),
        Some("的")
    );
    type_code(&mut session, "je");
    assert_eq!(
        session.process(Key::Space).unwrap().commit.as_deref(),
        Some("他")
    );
    assert!(!session.composing(), "二码单字上屏后不能残留编码接到下一码");
    // 单字全码后的下一键顶上首选，不能被当作空码清掉。
    type_code(&mut session, "tupg");
    let outcome = session.process(Key::Character('u')).unwrap();
    assert_eq!(outcome.commit.as_deref(), Some("我"));
    assert_eq!(outcome.state.candidates[0].text, "的");
}

#[test]
fn word_mode_applies_common_filter_to_every_character() {
    let dictionary = Dictionary::new(word_data()).unwrap();
    let options = Options {
        common_only: true,
        ..word_options()
    };
    let mut session = Session::new(&dictionary, options);
    assert_eq!(texts(&mut session, "wr"), Vec::<String>::new());
    assert_eq!(texts(&mut session, "fi"), ["一般", "一"]);
    // 含一个非常用字的词被滤掉；标点不是汉字，不参与过滤。
    assert_eq!(
        texts(&mut session, "wmfy"),
        ["头发", "喉头发", "甲，乙", "甲乙"]
    );
}

#[test]
fn word_mode_converts_output_form_like_opencc() {
    let dictionary = Dictionary::new(word_data()).unwrap();
    let traditional = Options {
        output_form: OutputForm::Traditional,
        ..word_options()
    };
    let mut session = Session::new(&dictionary, traditional);
    // 整词命中用短语，逐字取首选会错成“頭發”；未收录的词逐段取最长键的首选字形。
    let bucket = texts(&mut session, "wmfy");
    assert_eq!(bucket[0], "頭髮");
    assert_eq!(bucket[1], "喉頭髮");
    // 整词多字形全部列出；转换后与其他候选重复的只留首个。
    assert_eq!(texts(&mut session, "wmfz"), ["表面", "錶面"]);
    // 来源标注只属于单字。
    session.clear().unwrap();
    assert!(
        type_code_state(&mut session, "wmfy").candidates[0]
            .source
            .is_none()
    );
    let simplified = Options {
        output_form: OutputForm::Simplified,
        ..word_options()
    };
    let mut session = Session::new(&dictionary, simplified);
    assert!(texts(&mut session, "wmfy").contains(&"乾隆".to_owned()));
}

#[test]
fn word_mode_shows_learning_for_characters_and_pinyin_only() {
    let mut data = word_data();
    data.details = vec![Detail {
        character: '的',
        full_codes: vec!["unid"],
        decomposition: Some("白勹丶"),
        readings: Vec::new(),
    }];
    data.entries.push(Entry {
        character: '的',
        code: "u",
        weight: 1,
    });
    data.pinyin = vec![Pronunciation {
        character: '的',
        pinyin: "de",
        weight: 1,
    }];
    let dictionary = Dictionary::new(data).unwrap();
    let mut session = Session::new(
        &dictionary,
        Options {
            show_learning: true,
            ..word_options()
        },
    );
    let state = type_code_state(&mut session, "u");
    assert_eq!(state.candidates[0].comment, "〔白勹丶 · u〕");
    assert_eq!(state.candidates[1].comment, "");
    // 词模式下的拼音反查同样要给出虎码，否则反查失去意义。
    let mut session = Session::new(&dictionary, word_options());
    session.process(Key::Character('`')).unwrap();
    let state = type_code_state(&mut session, "de");
    assert_eq!(state.candidates[0].comment, "〔白勹丶 · u〕");
}

#[test]
fn lookup_by_word_code_lists_words_but_not_single_characters() {
    let dictionary = Dictionary::new(word_data()).unwrap();
    let page = dictionary.lookup("u", 0).unwrap();
    let items: Vec<_> = page.items.iter().map(|item| item.text.as_str()).collect();
    assert_eq!(items, ["工作"]);
}

#[test]
fn word_tables_reject_invalid_rows_and_oversized_buckets() {
    let mut bad = word_data();
    bad.word_entries[0].code = "toolong";
    assert!(Dictionary::new(bad).is_err());
    let mut bad = word_data();
    bad.phrases[0].text = "一";
    assert!(Dictionary::new(bad).is_err());
    // 单字一对多展开后超过上限时，须在加载时拒绝而不是查询时截断。
    let mut bad = word_data();
    bad.word_entries = (0..4000)
        .map(|index| Entry {
            character: char::from_u32(0x4E00 + index).unwrap(),
            code: "u",
            weight: 1,
        })
        .collect();
    bad.traditional = bad
        .word_entries
        .iter()
        .map(|entry| mapping(entry.character.to_string().leak(), &["甲", "乙", "丙"]))
        .collect();
    assert!(Dictionary::new(bad).is_err());
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

#[test]
fn large_code_bucket_rebuild_stays_bounded_under_pressure() {
    const MATCHES: usize = 8_192;
    let entries: Vec<_> = (0..MATCHES)
        .map(|index| Entry {
            character: char::from_u32(0x4E00 + index as u32).unwrap(),
            code: "u",
            weight: index as u64,
        })
        .collect();
    let common = entries.iter().map(|entry| entry.character).collect();
    let dictionary = Dictionary::new(Data {
        entries,
        common,
        ..Default::default()
    })
    .unwrap();
    let mut session = Session::new(&dictionary, Options::default());
    let first = session.process(Key::Character('u')).unwrap().state;
    assert_eq!(first.page_index, 0);
    let expected_first = char::from_u32(0x4E00 + MATCHES as u32 - 1)
        .unwrap()
        .to_string();
    assert_eq!(
        first.candidates.first().map(|item| item.text.as_str()),
        Some(expected_first.as_str())
    );
    assert!(!first.is_last_page, "大桶首屏不能被错误标记为末页");
    let page = usize::from(Options::default().page_size);
    let last_page = (MATCHES - 1) / page;
    for _ in 0..last_page {
        session.process(Key::PageDown).unwrap();
    }
    let last = session.snapshot();
    assert_eq!(last.page_index, last_page);
    assert!(last.is_last_page);
    assert_eq!(last.candidates.len(), (MATCHES - 1) % page + 1);
    let expected_last = char::from_u32(0x4E00).unwrap().to_string();
    assert_eq!(
        last.candidates.last().map(|item| item.text.as_str()),
        Some(expected_last.as_str())
    );
    session.process(Key::Backspace).unwrap();
    let started = std::time::Instant::now();
    for _ in 0..100 {
        // 覆盖真实短码桶的最坏分支：每次重建仍保留完整分页集合，不静默截断候选。
        session.process(Key::Character('u')).unwrap();
        session.process(Key::Backspace).unwrap();
    }
    let elapsed = started.elapsed();
    eprintln!("large bucket pressure: n=100 rebuilds={elapsed:?}");
    assert!(
        elapsed <= std::time::Duration::from_secs(1),
        "短码桶重建过慢: {elapsed:?}"
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
fn reloaded_settings_wait_for_the_composition_to_end_including_a_top_up_and_backspace() {
    let dictionary = Dictionary::new(data()).unwrap();
    let traditional = Options {
        output_form: OutputForm::Traditional,
        ..Default::default()
    };
    let mut session = Session::new(&dictionary, Options::default());
    type_code(&mut session, "srid");
    session.adopt(traditional).unwrap();
    // 组合中不重算已显示的候选。
    let shown = session.snapshot();
    assert_eq!(shown.options, Options::default());
    assert_eq!(shown.candidates[0].text, "发");
    // 第五码顶屏：旧字按已显示的候选上屏，新组合换上新设置。
    let topped = session.process(Key::Character('u')).unwrap();
    assert_eq!(topped.commit.as_deref(), Some("发"));
    assert_eq!(topped.state.composition.text, "u");
    assert_eq!(topped.state.options, traditional);
    // 退格删空同样算组合结束。
    session.adopt(Options::default()).unwrap();
    assert_eq!(session.snapshot().options, traditional);
    let emptied = session.process(Key::Backspace).unwrap();
    assert!(!emptied.state.composing);
    assert_eq!(emptied.state.options, Options::default());
    // 无组合时立即换上。
    session.adopt(traditional).unwrap();
    assert_eq!(session.snapshot().options, traditional);
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
    assert_eq!(dictionary.lookup("NÜ", 0).unwrap().items[0].text, "女");
    let results = dictionary.lookup("的𠀀☃的", 0).unwrap();
    assert_eq!(results.items.len(), 3);
    assert!(results.items[2].learning.codes.is_empty());
    assert_eq!(results.items[0].learning.full_codes, vec!["unid"]);
    assert_eq!(session.snapshot(), before);
    assert!(dictionary.lookup(&"的".repeat(65), 0).is_err());
    // 连写是合法的词拼音，查不到只是空结果；超过连写上限才算输入错误。
    assert!(dictionary.lookup("nihaoma", 0).unwrap().items.is_empty());
    assert!(
        dictionary
            .lookup(&"a".repeat(MAX_PINYIN_BYTES + 1), 0)
            .is_err()
    );
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
            code: "a",
            weight: 100 - index as u64,
        });
        data.common.push(character);
    }
    let dictionary = Dictionary::new(data).unwrap();
    let nth = |n: u32| char::from_u32(0x4e10 + n).unwrap().to_string();
    // 默认每页 5 个：本页外的数字键静默忽略，不选到下一页；分号、引号按本页第二、三个。
    let mut session = Session::new(&dictionary, Options::default());
    type_code(&mut session, "a");
    let first = session.snapshot();
    assert_eq!(first.page_size, 5);
    assert_eq!(first.candidates.len(), 5);
    let outcome = session.process(Key::Character('7')).unwrap();
    assert!(outcome.handled && outcome.commit.is_none() && outcome.state.composing);
    assert_eq!(
        session.process(Key::Character('5')).unwrap().commit,
        Some(nth(4))
    );
    type_code(&mut session, "a");
    session.process(Key::PageDown).unwrap();
    assert_eq!(
        session.process(Key::Character(';')).unwrap().commit,
        Some(nth(6))
    );
    type_code(&mut session, "a");
    for _ in 0..4 {
        session.process(Key::PageDown).unwrap();
    }
    let last = session.snapshot();
    assert!(last.is_last_page && last.page_index == 4 && last.candidates.len() == 1);
    assert!(
        session
            .process(Key::Character('\''))
            .unwrap()
            .commit
            .is_none()
    );
    session.process(Key::Escape).unwrap();

    let mut session = Session::new(
        &dictionary,
        Options {
            page_size: 9,
            ..Options::default()
        },
    );
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
    // 越界翻页忽略：首页向上、末页向下都不改高亮，随后空格提交的仍是原高亮项。
    use Key::{Character, Next, PageDown, PageUp};
    for (moves, page, expected) in [
        (vec![Next, Next, PageUp], 0, 2),
        (vec![Next, Next, Character('-')], 0, 2),
        (vec![PageDown, PageDown, Next, Next, PageDown], 2, 20),
        (vec![PageDown, PageDown, Next, Next, Character('=')], 2, 20),
    ] {
        type_code(&mut session, "a");
        for key in moves {
            session.process(key).unwrap();
        }
        assert_eq!(session.snapshot().page_index, page);
        assert_eq!(
            session.process(Key::Space).unwrap().commit,
            Some(nth(expected))
        );
    }
}
#[test]
fn malformed_data_is_rejected_before_sessions_and_initialization_is_repeatable() {
    let mut bad = data();
    bad.entries[0].code = "toolong";
    assert!(Dictionary::new(bad).is_err());
    let mut bad = data();
    bad.pinyin[0].pinyin = "xi ng";
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
        code: "u",
        weight: 100,
    });
    data.entries.push(Entry {
        character: '的',
        code: "u",
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

#[test]
fn pinyin_lookup_finds_words_and_shows_their_codes() {
    let word = |text, pinyin: &str, weight| WordPinyin {
        text,
        pinyin: pinyin.to_owned(),
        weight,
    };
    let phrase = |text, code, weight| Phrase { text, code, weight };
    let source = Data {
        entries: vec![Entry {
            character: '先',
            code: "xi",
            weight: 1,
        }],
        pinyin: vec![Pronunciation {
            character: '先',
            pinyin: "xian",
            weight: 500,
        }],
        phrases: vec![
            phrase("西安", "xaan", 9),
            phrase("头发", "tofa", 9),
            phrase("嘀嘀", "didi", 9),
        ],
        word_pinyin: vec![
            word("西安", "xian", 900),
            word("头发", "toufa", 10),
            word("嘀嘀", "didi", 5),
        ],
        traditional: vec![mapping("头发", &["頭髮"])],
        ..Default::default()
    };
    let dictionary = Dictionary::new(source.clone()).unwrap();
    let typed = |options, keys: &str| {
        let mut session = Session::new(&dictionary, options);
        for key in keys.chars() {
            session.process(Key::Character(key)).unwrap();
        }
        session
    };
    // 同一拼音串字词按权重混排，词带词码，单字仍是拆分和编码。
    let mut session = typed(Options::default(), "`xian");
    let state = session.snapshot();
    let texts: Vec<_> = state.candidates.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["西安", "先"]);
    assert_eq!(state.candidates[0].comment, "〔xaan〕");
    assert_eq!(state.candidates[1].comment, "〔xi〕");
    assert_eq!(
        session.process(Key::Space).unwrap().commit.as_deref(),
        Some("西安")
    );
    // 繁体模式上屏转换后的词，词码仍按原词查到。
    let traditional = Options {
        output_form: OutputForm::Traditional,
        ..Default::default()
    };
    let state = typed(traditional, "`toufa").snapshot();
    assert_eq!(state.candidates[0].text, "頭髮");
    assert_eq!(state.candidates[0].comment, "〔tofa〕");
    // 只反查时才显示：打词的候选不带编码，查字窗走同一张表。
    let words = Options {
        typing_mode: TypingMode::Word,
        auto_commit_four: false,
        common_only: false,
        ..Default::default()
    };
    assert_eq!(typed(words, "tofa").snapshot().candidates[0].comment, "");
    let page = dictionary.lookup("toufa", 0).unwrap();
    assert_eq!(page.items[0].text, "头发");
    assert_eq!(page.items[0].learning.codes, ["tofa"]);
    // 词码和拼音恰好同串时，查字结果不重复。
    assert_eq!(dictionary.lookup("didi", 0).unwrap().items.len(), 1);
    // 连写上限内的键都进组合，超出的不再加入。
    let long = typed(
        Options::default(),
        &format!("`{}", "a".repeat(MAX_PINYIN_BYTES + 3)),
    );
    assert_eq!(long.snapshot().composition.text.len(), MAX_PINYIN_BYTES + 1);
    for bad in [word("西安", "xi an", 1), word("西", "xi", 1)] {
        let mut data = source.clone();
        data.word_pinyin = vec![bad];
        assert!(Dictionary::new(data).is_err());
    }
}
