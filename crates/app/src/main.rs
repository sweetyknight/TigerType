//! 离线资源与核心诊断入口；不注册输入法、不启动旧引擎或探测个人词库。
use std::path::Path;
use tigerrime_app::{Result, import, project_root};
use tigerrime_contracts::{Key, Options};
use tigerrime_engine::{Dictionary, Session};

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args == ["--help"] {
        println!(
            "TigerRime Rust 核心工具\n  import-resources <新数据包绝对路径>\n  verify-package <数据包绝对路径>\n  lookup <数据包绝对路径> <拼音或汉字>\n  demo-input <数据包绝对路径> <编码>\n尚不安装或注册输入法。"
        );
        return Ok(());
    }
    match (args[0].to_str(), args.len()) {
        (Some("import-resources"), 2) => {
            let package = import::build(&project_root()?.join("vendor/tiger"))?;
            import::save(&package, Path::new(&args[1]))?;
            println!(
                "已校验并写入 {} 条单字编码；未修改原资源。",
                package.data.entries.len()
            );
        }
        (Some("verify-package"), 2) => {
            let package = import::load(Path::new(&args[1]))?;
            println!(
                "数据包校验通过：{} 条编码，{} 条单字拼音，{} 条学习资料。",
                package.data.entries.len(),
                package.data.pinyin.len(),
                package.data.details.len()
            );
        }
        (Some("lookup" | "demo-input"), 3) => {
            let package = import::load(Path::new(&args[1]))?;
            let dictionary = Dictionary::new(package.data)?;
            let query = args[2].to_str().ok_or("查询不是有效 Unicode")?;
            if args[0] == "lookup" {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&dictionary.lookup(query, 0)?)?
                );
            } else {
                let mut session = Session::new(&dictionary, Options::default());
                for c in query.chars() {
                    let outcome = session.process(if c == ' ' {
                        Key::Space
                    } else {
                        Key::Character(c)
                    })?;
                    println!("{}", serde_json::to_string(&outcome)?);
                }
            }
        }
        _ => return Err("无效参数；请运行 --help".into()),
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
