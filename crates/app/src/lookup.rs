//! 后台进程内查字/设置的固定工作线程控制器：与引擎线程共用只读词表，独占受管目录租约；
//! 不创建输入会话，查询不产生上屏提交。
use crate::{Result, data::Data};
use sha2::{Digest, Sha256};
use tigertype_contracts::{
    Options,
    ui::{Reply, Request},
};
use tigertype_engine::Dictionary;

/// 受管设置独占归工作线程；窗口只接收拥有所有权的 contracts 数据。
pub(crate) struct Controller<'a> {
    dictionary: &'a Dictionary,
    data: Data,
    options: Options,
    digest: Option<String>,
}
impl<'a> Controller<'a> {
    pub(crate) fn new(dictionary: &'a Dictionary, data: Data) -> Result<Self> {
        let bytes = data.settings()?;
        let options: Options = bytes
            .as_deref()
            .map(serde_json::from_slice)
            .transpose()?
            .unwrap_or_default();
        options.validate()?;
        let digest = bytes.map(|bytes| format!("{:x}", Sha256::digest(bytes)));
        Ok(Self {
            dictionary,
            data,
            options,
            digest,
        })
    }
    pub(crate) fn options(&self) -> Options {
        self.options
    }
    pub(crate) fn execute(&mut self, request: Request) -> Reply {
        match request {
            Request::Lookup { query, page } => Reply::Lookup(
                self.dictionary
                    .lookup(&query, page)
                    .map_err(|e| e.to_string()),
            ),
            Request::SaveSettings(options) => {
                Reply::Saved(self.save(options).map_err(|e| e.to_string()))
            }
        }
    }
    fn save(&mut self, options: Options) -> Result<Options> {
        let bytes = serde_json::to_vec_pretty(&options)?;
        self.data.replace_settings(self.digest.as_deref(), &bytes)?;
        // 只有事务替换成功才更新内存和期望摘要；失败不伪装成已保存。
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
        // 模拟外部改写：保存不得覆盖别人的内容，也不得改变已确认设置。
        let changed = serde_json::to_vec(&Options::default())?;
        std::fs::write(root.join("config/settings.json"), &changed)?;
        assert!(matches!(
            controller.execute(Request::SaveSettings(Options::default())),
            Reply::Saved(Err(_))
        ));
        assert_eq!(controller.options(), options);
        assert_eq!(std::fs::read(root.join("config/settings.json"))?, changed);
        drop(controller);
        let reopened = Controller::new(&dictionary, Data::open(&root)?)?;
        assert_eq!(reopened.options(), Options::default());
        Ok(())
    }
    #[test]
    fn invalid_saved_settings_are_not_silently_replaced() -> Result<()> {
        let dictionary = dictionary()?;
        let root =
            resources::new_directory(&std::env::temp_dir(), "tiger-lookup-invalid")?.join("data");
        let data = Data::create(&root)?;
        std::fs::write(root.join("config/settings.json"), b"not json")?;
        assert!(Controller::new(&dictionary, data).is_err());
        assert_eq!(
            std::fs::read(root.join("config/settings.json"))?,
            b"not json"
        );
        Ok(())
    }
}
