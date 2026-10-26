//! 完整锁定码表验收单列；不加载 DLL，不运行脚本，不接触个人词库。
use serde_json::json;
use std::time::Instant;
use tigertype_app::{Result, TABLES, import, project_root, resources};
use tigertype_contracts::{Key, Options, OutputForm, TypingMode};
use tigertype_engine::{Dictionary, Session};

fn verify(root: &std::path::Path) -> Result<serde_json::Value> {
    // 与后台启动同一路径：核对锁定清单、解析原文、建立词表。
    let started = Instant::now();
    let data = import::load(&root.join("resources").join(TABLES))?;
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
    let root = project_root()?;
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
