//! 查字/设置的固定工作线程控制器：与引擎线程共用只读词表，独占受管目录租约；
//! 不创建输入会话，查询不产生上屏提交。
use crate::{Result, settings, storage::Data};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tigertype_contracts::{
    Options,
    ui::{Reply, Request},
};
use tigertype_engine::Dictionary;

/// 受管设置独占归工作线程；窗口只接收拥有所有权的 contracts 数据。
pub struct Controller<'a> {
    dictionary: &'a Dictionary,
    data: Data,
    options: Options,
    digest: Option<String>,
}
impl<'a> Controller<'a> {
    pub fn new(dictionary: &'a Dictionary, data: Data) -> Result<Self> {
        let bytes = data.settings()?;
        let options = match &bytes {
            Some(bytes) => settings::parse(bytes)?,
            None => Options::default(),
        };
        let mut controller = Self {
            dictionary,
            data,
            options,
            digest: bytes.map(|bytes| format!("{:x}", Sha256::digest(bytes))),
        };
        // 首次运行即写出带注释的默认配置，“打开配置文件”与“重新载入”从此总有文件可用。
        if controller.digest.is_none() {
            controller.save(options)?;
        }
        Ok(controller)
    }
    pub fn options(&self) -> Options {
        self.options
    }
    /// 将数据层已规范化的配置路径交给设置页显示与打开，避免 UI 根据输入参数自行拼路径；
    /// 去掉 `\\?\` 前缀，用户看到的、编辑器收到的都是常见写法。
    pub fn settings_path(&self) -> PathBuf {
        crate::storage::plain(&self.data.settings_path())
    }
    pub fn execute(&mut self, request: Request) -> Reply {
        match request {
            Request::Lookup { query, page } => Reply::Lookup(
                self.dictionary
                    .lookup(&query, page)
                    .map_err(|e| e.to_string()),
            ),
            Request::SaveSettings(options) => {
                Reply::Saved(self.save(options).map_err(|e| e.to_string()))
            }
            Request::ReloadSettings => Reply::Saved(self.reload().map_err(|e| e.to_string())),
        }
    }
    fn save(&mut self, options: Options) -> Result<Options> {
        let bytes = settings::render(&options)?.into_bytes();
        self.data.replace_settings(self.digest.as_deref(), &bytes)?;
        // 只有事务替换成功才更新内存和期望摘要；失败不伪装成已保存。
        self.digest = Some(format!("{:x}", Sha256::digest(&bytes)));
        self.options = options;
        Ok(options)
    }
    /// 采纳用户手工修改：读回并校验通过才换上新设置与期望摘要，出错时保持当前设置。
    fn reload(&mut self) -> Result<Options> {
        let bytes = self
            .data
            .settings()?
            .ok_or("settings.toml 不存在；改动任一设置即会重新生成")?;
        let options = settings::parse(&bytes)?;
        self.digest = Some(format!("{:x}", Sha256::digest(&bytes)));
        self.options = options;
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources;
    use tigertype_contracts::OutputForm;
    use tigertype_engine::{Data as EngineData, Entry};

    fn dictionary() -> Result<Dictionary> {
        // 所有文件测试都使用一条合法合成码表，避免被空码表校验提前短路。
        Ok(Dictionary::new(EngineData {
            entries: vec![Entry {
                character: '𠀀',
                code: "ab".into(),
                weight: 1,
            }],
            ..Default::default()
        })?)
    }
    #[test]
    fn queries_and_transactional_settings_roundtrip_without_input_session() -> Result<()> {
        let dictionary = dictionary()?;
        let root =
            resources::new_directory(&std::env::temp_dir(), "tiger-lookup-test")?.join("data");
        let mut controller = Controller::new(&dictionary, Data::create(&root)?)?;
        let options = Options {
            output_form: OutputForm::Traditional,
            auto_commit_four: true,
            ..Default::default()
        };
        assert!(
            matches!(controller.execute(Request::SaveSettings(options)), Reply::Saved(Ok(saved)) if saved == options)
        );
        drop(controller);
        let mut controller = Controller::new(&dictionary, Data::open(&root)?)?;
        assert_eq!(controller.options(), options);
        let Reply::Lookup(Ok(page)) = controller.execute(Request::Lookup {
            query: "𠀀".into(),
            page: 0,
        }) else {
            panic!("需要查询结果");
        };
        assert_eq!(page.items[0].text, "𠀀");
        assert_eq!(page.items[0].learning.codes, ["ab"]);
        assert!(matches!(
            controller.execute(Request::Lookup {
                query: "字".repeat(65),
                page: 0
            }),
            Reply::Lookup(Err(_))
        ));
        // 模拟外部改写：保存不得覆盖别人的内容，也不得改变已确认设置；重新载入后才采纳。
        let path = root.join("config/settings.toml");
        let edited = Options {
            page_size: 7,
            ..Default::default()
        };
        let changed = settings::render(&edited)?;
        std::fs::write(&path, &changed)?;
        let Reply::Saved(Err(error)) =
            controller.execute(Request::SaveSettings(Options::default()))
        else {
            panic!("外部修改后保存必须被拒绝");
        };
        assert!(error.contains("重新载入"), "{error}");
        assert_eq!(controller.options(), options);
        assert_eq!(std::fs::read_to_string(&path)?, changed);
        assert!(
            matches!(controller.execute(Request::ReloadSettings), Reply::Saved(Ok(loaded)) if loaded == edited)
        );
        assert_eq!(controller.options(), edited);
        // 手工改坏：重新载入报出行号，当前设置与文件都不变；修好之前保存仍被拒绝。
        let broken = changed.replace("page_size = 7", "page_size = seven");
        std::fs::write(&path, &broken)?;
        let Reply::Saved(Err(error)) = controller.execute(Request::ReloadSettings) else {
            panic!("格式错误必须被拒绝");
        };
        assert!(error.contains("settings.toml 第"), "{error}");
        assert_eq!(controller.options(), edited);
        assert!(matches!(
            controller.execute(Request::SaveSettings(Options::default())),
            Reply::Saved(Err(_))
        ));
        assert_eq!(std::fs::read_to_string(&path)?, broken);
        std::fs::write(&path, &changed)?;
        assert!(matches!(
            controller.execute(Request::ReloadSettings),
            Reply::Saved(Ok(_))
        ));
        assert!(matches!(
            controller.execute(Request::SaveSettings(Options::default())),
            Reply::Saved(Ok(_))
        ));
        drop(controller);
        let reopened = Controller::new(&dictionary, Data::open(&root)?)?;
        assert_eq!(reopened.options(), Options::default());
        Ok(())
    }
    #[test]
    fn first_run_writes_the_commented_defaults_and_invalid_files_are_kept() -> Result<()> {
        let dictionary = dictionary()?;
        let root =
            resources::new_directory(&std::env::temp_dir(), "tiger-lookup-first")?.join("data");
        let controller = Controller::new(&dictionary, Data::create(&root)?)?;
        let written = std::fs::read(root.join("config/settings.toml"))?;
        assert_eq!(settings::parse(&written)?, Options::default());
        assert_eq!(controller.options(), Options::default());
        // 设置页显示并交给编辑器的路径不带 canonicalize 的 `\\?\` 前缀，且仍指向同一文件。
        let shown = controller.settings_path();
        assert!(!shown.to_string_lossy().starts_with(r"\\?\"), "{shown:?}");
        assert_eq!(std::fs::read(&shown)?, written);
        drop(controller);
        let root =
            resources::new_directory(&std::env::temp_dir(), "tiger-lookup-invalid")?.join("data");
        let data = Data::create(&root)?;
        std::fs::write(root.join("config/settings.toml"), b"not toml")?;
        assert!(Controller::new(&dictionary, data).is_err());
        assert_eq!(
            std::fs::read(root.join("config/settings.toml"))?,
            b"not toml"
        );
        Ok(())
    }
}
