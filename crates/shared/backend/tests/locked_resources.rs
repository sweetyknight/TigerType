//! 完整锁定码表验收单列；不加载 DLL，不运行脚本，不接触个人词库。
use serde_json::json;
use std::time::Instant;
use tigertype_backend::{Data, Result, import, resources};
use tigertype_contracts::{Key, Options, OutputForm, TypingMode};
use tigertype_engine::{Dictionary, Session};

/// 词模式对照官方词版：样例取自官方教程与 tigress 词库的权重结果，字码全集须与单字表一致。
fn verify_word_mode(dictionary: &Dictionary, data: &Data) -> Result<()> {
    let options = |common_only, output_form| Options {
        typing_mode: TypingMode::Word,
        common_only,
        output_form,
        ..Default::default()
    };
    let first_page = |code: &str, options| -> Result<Vec<String>> {
        let mut session = Session::new(dictionary, options);
        for character in code.chars() {
            session.process(Key::Character(character))?;
        }
        let state = session.snapshot();
        Ok(state.candidates.into_iter().map(|c| c.text).collect())
    };
    let original = options(true, OutputForm::Original);
    for (code, expected) in [
        ("u", &["的", "工作"][..]),
        ("a", &["来", "那个"]),
        ("f", &["一", "开始"]),
        ("i", &["为", "为什么"]),
        ("w", &["得", "怎么"]),
        ("fi", &["一般", "一"]),
        ("kk", &["治", "渐渐"]),
        ("oo", &["州", "尴尬"]),
        ("vj", &["以", "己"]),
        ("yp", &["小米", "轼"]),
        ("je", &["他"]),
        ("unid", &["的"]),
        ("tupg", &["我"]),
        ("ugjw", &["工作"]),
        ("rcp", &["对不起"]),
    ] {
        assert_eq!(first_page(code, original)?, expected, "{code}");
    }
    // 这些码只有词版权重才让简词居首，用单字表权重合并会错。
    assert_eq!(first_page("bj", original)?[0], "好像");
    assert_eq!(first_page("ez", original)?[0], "本身");
    // 含非常用字的词被官方常用字过滤滤掉，全字集才出现。
    let rare = options(false, OutputForm::Original);
    assert!(first_page("ejml", rare)?.iter().any(|t| t == "树倒猢猻散"));
    assert!(
        !first_page("ejml", original)?
            .iter()
            .any(|t| t == "树倒猢猻散")
    );
    let code = data.phrases.iter().find(|p| p.text == "头发").unwrap();
    let traditional = options(true, OutputForm::Traditional);
    assert!(first_page(&code.code, traditional)?.contains(&"頭髮".to_owned()));

    let pairs = |rows: &[tigertype_backend::Entry]| {
        let mut pairs: Vec<_> = rows.iter().map(|e| (e.code.clone(), e.character)).collect();
        pairs.sort();
        pairs.dedup();
        pairs
    };
    assert_eq!(pairs(&data.word_entries), pairs(&data.entries));
    // 全字集下每个字都要能在词模式打出，翻页查找，不只看首页。
    let mut session = Session::new(dictionary, rare);
    for entry in &data.word_entries {
        session.clear()?;
        for character in entry.code.chars() {
            session.process(Key::Character(character))?;
        }
        let text = entry.character.to_string();
        let mut state = session.snapshot();
        while !state.candidates.iter().any(|c| c.text == text) {
            assert!(!state.is_last_page, "{text} {} 打不出", entry.code);
            state = session.process(Key::PageDown)?.state;
        }
    }
    Ok(())
}

fn verify(root: &std::path::Path) -> Result<serde_json::Value> {
    // 与后台启动同一路径：核对锁定清单、解析原文、建立词表。
    let started = Instant::now();
    let data = import::load(&root.join("resources/tiger"))?;
    let dictionary = Dictionary::new(data.clone())?;
    let load_ms = started.elapsed().as_millis();
    let mut missing_full_codes = 0;
    // 全表交叉检查，不只对几个演示字声称数据导入正确。
    for entry in &data.entries {
        assert!(
            dictionary
                .learning(entry.character)
                .codes
                .contains(&entry.code)
        );
    }
    for detail in &data.details {
        let codes = dictionary.learning(detail.character).codes;
        missing_full_codes += detail
            .full_codes
            .iter()
            .filter(|code| !codes.contains(code))
            .count();
    }
    for code in ["u", "uni", "unid"] {
        let mut session = Session::new(&dictionary, Options::default());
        for character in code.chars() {
            assert!(session.process(Key::Character(character))?.commit.is_none());
        }
        assert_eq!(session.process(Key::Space)?.commit.as_deref(), Some("的"));
    }
    let mut session = Session::new(
        &dictionary,
        Options {
            output_form: OutputForm::Traditional,
            show_learning: true,
            ..Default::default()
        },
    );
    for character in "kry".chars() {
        session.process(Key::Character(character))?;
    }
    assert_eq!(session.snapshot().candidates[0].text, "漢");
    assert!(
        session.snapshot().candidates[0]
            .learning
            .as_ref()
            .unwrap()
            .codes
            .contains(&"kldh".into())
    );
    assert!(
        dictionary
            .lookup("nv", 0)?
            .items
            .iter()
            .any(|item| item.text == "女")
    );
    assert_eq!(dictionary.lookup("nü", 0)?, dictionary.lookup("nv", 0)?);
    let mut words = Session::new(
        &dictionary,
        Options {
            typing_mode: TypingMode::Word,
            ..Default::default()
        },
    );
    for character in "tuja".chars() {
        words.process(Key::Character(character))?;
    }
    assert_eq!(words.snapshot().candidates[0].text, "我们");
    assert!(
        dictionary
            .lookup("我们", 0)?
            .items
            .iter()
            .any(|item| item.text == "我们")
    );
    verify_word_mode(&dictionary, &data)?;
    assert!(!dictionary.learning('𠀀').codes.is_empty());
    Ok(
        json!({"entries":data.entries.len(),"phrases":data.phrases.len(),"pinyin":data.pinyin.len(),
        "details":data.details.len(),"full_codes_not_directly_inputtable":missing_full_codes,
        "source_count":import::SOURCES.len(),"load_ms":load_ms}),
    )
}

#[test]
#[ignore = "完整码表验收单列；必须显式执行，不能把 ignored 计为通过"]
fn locked_text_resources_build_and_regress_without_rime() -> Result<()> {
    // 源码工作区根：码表在 resources/tiger，证据写入 artifacts（不入库）。
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()?;
    let report = resources::new_directory(&root.join("artifacts"), "rust-tiger")?;
    let result = verify(&root);
    let evidence = match &result {
        Ok(details) => json!({"passed":true,"details":details}),
        Err(error) => json!({"passed":false,"error":error.to_string()}),
    };
    std::fs::write(
        report.join("result.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    println!("完整资源证据：{}", report.display());
    result.map(|_| ())
}
