//! 独立查字工具组装：固定 UI/工作线程；不连接 IPC、不创建输入会话、不接管 APPDATA。
use crate::{Result, data::Data, import};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::mpsc, thread};
use tigerrime_contracts::{
    Options,
    ui::{Reply, Request},
};
use tigerrime_engine::Dictionary;

/// 引擎和受管设置独占归工作线程；窗口只接收拥有所有权的 contracts 数据。
struct Controller {
    dictionary: Dictionary,
    data: Data,
    options: Options,
    digest: Option<String>,
}
impl Controller {
    fn new(dictionary: Dictionary, data: Data) -> Result<Self> {
        let bytes = data.settings()?;
        let options = bytes
            .as_deref()
            .map(serde_json::from_slice)
            .transpose()?
            .unwrap_or_default();
        let digest = bytes.map(|bytes| format!("{:x}", Sha256::digest(bytes)));
        Ok(Self {
            dictionary,
            data,
            options,
            digest,
        })
    }
    fn options(&self) -> Options {
        self.options
    }
    fn execute(&mut self, request: Request) -> Reply {
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

pub fn run(package: &Path, directory: &Path) -> Result<()> {
    // 启动时完成包校验和索引构建；显式目录不存在才创建，已有目录必须通过所有权检查。
    let dictionary = Dictionary::new(import::load(package)?.data)?;
    let data = if directory.try_exists()? {
        Data::open(directory)?
    } else {
        Data::create(directory)?
    };
    let mut controller = Controller::new(dictionary, data)?;
    let options = controller.options();
    let (requests, receiver) = mpsc::sync_channel(1);
    let (replies, inbox) = tigerrime_ui::mailbox()?;
    let worker = thread::Builder::new()
        .name("tiger-lookup-engine".into())
        .spawn(move || {
            while let Ok(request) = receiver.recv() {
                if replies.send(controller.execute(request)).is_err() {
                    break;
                }
            }
        })?;
    let result = tigerrime_ui::run(requests, inbox, options);
    // UI 退出先断开有界请求通道；真正 join 后才确认线程/租约释放，不用超时冒充终止。
    worker.join().map_err(|_| "查字工作线程异常退出")?;
    result.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources;
    use tigerrime_contracts::OutputForm;
    use tigerrime_engine::{Data as EngineData, Entry};

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
        let root =
            resources::new_directory(&std::env::temp_dir(), "tiger-lookup-test")?.join("data");
        let mut controller = Controller::new(dictionary()?, Data::create(&root)?)?;
        let options = Options {
            output_form: OutputForm::Traditional,
            auto_commit_four: true,
            ..Default::default()
        };
        assert!(
            matches!(controller.execute(Request::SaveSettings(options)), Reply::Saved(Ok(saved)) if saved == options)
        );
        drop(controller);
        let mut controller = Controller::new(dictionary()?, Data::open(&root)?)?;
        assert_eq!(controller.options(), options);
        let Reply::Lookup(Ok(page)) = controller.execute(Request::Lookup {
            query: "𠀀".into(),
            page: 0,
        }) else {
            panic!("需要查询结果");
        };
        assert_eq!(page.items[0].character, '𠀀');
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
        let reopened = Controller::new(dictionary()?, Data::open(&root)?)?;
        assert_eq!(reopened.options(), Options::default());
        Ok(())
    }
    #[test]
    fn invalid_saved_settings_are_not_silently_replaced() -> Result<()> {
        let root =
            resources::new_directory(&std::env::temp_dir(), "tiger-lookup-invalid")?.join("data");
        let data = Data::create(&root)?;
        std::fs::write(root.join("config/settings.json"), b"not json")?;
        assert!(Controller::new(dictionary()?, data).is_err());
        assert_eq!(
            std::fs::read(root.join("config/settings.json"))?,
            b"not json"
        );
        Ok(())
    }
}
