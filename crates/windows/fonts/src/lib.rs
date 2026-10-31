//! 随包文楷的私有加载边界；仅在 COM 构造/UI 启动调用，按键、布局与绘制不得读文件。
use std::path::{Path, PathBuf};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const FAMILY: &str = "LXGW WenKai GB Screen";
const SOURCE: &str = "LXGWWenKaiGBScreen.ttf";

fn locked() -> Result<(u64, String)> {
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../../../../resources/fonts.lock.json"))?;
    let fonts = lock["fonts"].as_array().ok_or("字体清单格式错误")?;
    let entries: Vec<_> = fonts.iter().filter(|f| f["path"] == SOURCE).collect();
    if entries.len() != 1 || entries[0]["family"] != FAMILY {
        return Err("字体清单缺少唯一的虎码文楷".into());
    }
    let size = entries[0]["size"]
        .as_u64()
        .filter(|s| (1..=128 * 1024 * 1024).contains(s))
        .ok_or("字体大小无效")?;
    let hash = entries[0]["sha256"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("字体摘要无效")?;
    Ok((size, hash.to_owned()))
}
/// 按摘要命名，不就地改写仍被旧宿主持有的字体文件。
pub fn installed_name() -> Result<String> {
    Ok(format!("LXGWWenKaiGBScreen-{}.ttf", locked()?.1))
}
pub fn bundled_path(module: &Path) -> Result<PathBuf> {
    // 开发构建只用编译时确定的仓库资源；发布构建只用自身目录，不搜索个人字体或当前目录。
    if cfg!(debug_assertions) {
        Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../resources/fonts")
            .join(SOURCE))
    } else {
        Ok(module.with_file_name(installed_name()?))
    }
}
#[cfg(windows)]
mod native;
#[cfg(windows)]
pub use native::PrivateFont;
