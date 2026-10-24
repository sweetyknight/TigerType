//! 完整锁定文本资源验收单列；不加载 DLL，不运行脚本，不接触个人词库。
use serde_json::json;
use tigertype_app::{Result, import, project_root, resources};
use tigertype_contracts::{Key, Options, OutputForm};
use tigertype_engine::{Dictionary, Session};

fn verify(root: &std::path::Path, work: &std::path::Path) -> Result<serde_json::Value> {
    let package = import::build(&root.join("vendor/tiger"))?;
    let dictionary = Dictionary::new(package.data.clone())?;
    let mut missing_full_codes = 0;
    // 全表交叉检查，不只对几个演示字声称数据导入正确。
    for entry in &package.data.entries {
        assert!(
            dictionary
                .learning(entry.character)
                .codes
                .contains(&entry.code)
        );
    }
    for detail in &package.data.details {
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
            .any(|item| item.character == '女')
    );
    assert_eq!(dictionary.lookup("nü", 0)?, dictionary.lookup("nv", 0)?);
    assert!(!dictionary.learning('𠀀').codes.is_empty());
    let target = work.join("tiger.json");
    import::save(&package, &target)?;
    let digest = resources::sha256(&target)?;
    assert!(import::save(&package, &target).is_err());
    assert_eq!(resources::sha256(&target)?, digest);
    let mut loaded = import::load(&target)?;
    loaded.data.entries[0].weight += 1;
    let damaged = work.join("damaged.json");
    std::fs::write(&damaged, serde_json::to_vec(&loaded)?)?;
    assert!(import::load(&damaged).is_err());
    assert_eq!(resources::sha256(&target)?, digest);
    Ok(
        json!({"entries":package.data.entries.len(),"pinyin":package.data.pinyin.len(),
        "details":package.data.details.len(),"full_codes_not_directly_inputtable":missing_full_codes,
        "package_sha256":digest,"source_count":package.sources.len()}),
    )
}

#[test]
#[ignore = "需要本地已锁定资源；必须显式执行，不能把 ignored 计为通过"]
fn locked_text_resources_build_and_regress_without_rime() -> Result<()> {
    let root = project_root()?;
    let report = resources::new_directory(&root.join("artifacts"), "rust-tiger")?;
    let work =
        resources::new_directory(&std::env::temp_dir().join("TigerType-tests"), "rust-tiger")?;
    if work.canonicalize()?.starts_with(&root) {
        return Err("测试运行目录必须在源码之外".into());
    }
    let result = verify(&root, &work);
    let evidence = match &result {
        Ok(details) => json!({"passed":true,"work":work,"details":details}),
        Err(error) => json!({"passed":false,"work":work,"error":error.to_string()}),
    };
    std::fs::write(
        report.join("result.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    println!("完整资源证据：{}", report.display());
    result.map(|_| ())
}
