//! 门禁命令矩阵：本机执行、异平台只检查类型，不把不可运行的测试计为通过。
const MAC: &[&str] = &[
    "tigertype-contracts",
    "tigertype-engine",
    "tigertype-input-client",
    "tigertype-ui-model",
    "tigertype-backend",
    "tigertype-macos-input-method",
    "tigertype-macos-ui",
    "tigertype-macos-app",
    // 样式表是纯逻辑，Mac 上同样执行；GTK 控件只在 Linux 编译，原生测试在 Mac 上报告未运行。
    "tigertype-linux-ui",
    "tigertype-linux-input-method",
    "tigertype-linux-app",
    "tigertype-dev",
];
const LINUX: &[&str] = &[
    "tigertype-contracts",
    "tigertype-engine",
    "tigertype-input-client",
    "tigertype-ui-model",
    "tigertype-backend",
    "tigertype-linux-ui",
    "tigertype-linux-input-method",
    "tigertype-linux-app",
    "tigertype-dev",
];
fn selection(os: &str) -> Vec<&'static str> {
    match os {
        "macos" => MAC.iter().flat_map(|package| ["-p", *package]).collect(),
        "linux" => LINUX.iter().flat_map(|package| ["-p", *package]).collect(),
        "windows" => vec!["--workspace"],
        _ => panic!("不支持的检查宿主"),
    }
}
pub fn commands(os: &str) -> Vec<Vec<&'static str>> {
    let mut commands = vec![
        vec![
            "test",
            "--locked",
            "--offline",
            "-p",
            "tigertype-dev",
            "--test",
            "governance",
        ],
        vec!["fmt", "--all", "--", "--check"],
    ];
    // 编辑器默认检查整个工作区；本机门禁也覆盖异平台入口及依赖的条件声明。
    commands.push(vec![
        "clippy",
        "--locked",
        "--offline",
        "--all-targets",
        "--workspace",
        "--",
        "-D",
        "warnings",
    ]);
    // 明确测试种类，避免 Windows 重链接仍在运行的 check.exe；all-targets Clippy 仍覆盖示例。
    if os == "linux" {
        commands.push(vec![
            "run",
            "--locked",
            "--offline",
            "-p",
            "tigertype-dev",
            "--example",
            "fcitx_plugin",
        ]);
    }
    let native_targets: &[Option<&str>] = if os == "windows" {
        &[None, Some("i686-pc-windows-msvc")]
    } else {
        &[None]
    };
    for target in native_targets {
        for kinds in [&["--lib", "--bins", "--tests"][..], &["--doc"][..]] {
            let mut test = vec!["test", "--locked", "--offline"];
            test.extend(selection(os));
            test.extend(kinds);
            if let Some(target) = target {
                test.extend(["--target", target]);
            }
            commands.push(test);
        }
    }
    // Linux 原生 GTK 检查不可由异平台类型检查代替；只要求本机已准备的 Linux 工具链。
    let targets: &[(&str, &str)] = match os {
        "macos" => &[
            ("windows", "x86_64-pc-windows-msvc"),
            ("windows", "i686-pc-windows-msvc"),
        ],
        "windows" => &[
            ("macos", "aarch64-apple-darwin"),
            ("macos", "x86_64-apple-darwin"),
        ],
        "linux" => &[],
        _ => panic!("不支持的检查宿主"),
    };
    for (platform, target) in targets {
        let mut lint = vec!["clippy", "--locked", "--offline", "--all-targets"];
        lint.extend(selection(platform));
        lint.extend(["--target", target, "--", "-D", "warnings"]);
        commands.push(lint);
    }
    commands
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_matrix_runs_only_native_tests_and_checks_all_mac_product_and_tool_targets() {
        let win = commands("windows");
        let mac = commands("macos");
        let linux = commands("linux");
        for commands in [&win, &mac, &linux] {
            let native_lint = commands
                .iter()
                .find(|c| c[0] == "clippy" && !c.contains(&"--target"))
                .unwrap();
            assert!(native_lint.contains(&"--workspace"));
            assert!(native_lint.contains(&"--all-targets"));
        }
        assert!(
            win.iter()
                .any(|c| c[0] == "test" && c.contains(&"i686-pc-windows-msvc"))
        );
        for target in ["aarch64-apple-darwin", "x86_64-apple-darwin"] {
            let command = win.iter().find(|c| c.contains(&target)).unwrap();
            assert_eq!(command[0], "clippy");
            for package in MAC {
                assert!(command.contains(package));
            }
            assert!(command.contains(&"--all-targets"));
        }
        for command in mac.iter().filter(|c| c[0] == "test") {
            assert!(!command.contains(&"--target"));
            assert!(!command.contains(&"tigertype-windows-app"));
        }
        assert_eq!(
            mac.iter()
                .filter(|c| c[0] == "clippy" && c.contains(&"--target"))
                .count(),
            2
        );
        assert!(linux.iter().all(|c| !c.contains(&"--target")));
        for command in linux.iter().filter(|c| c.contains(&"--lib")) {
            assert!(command.contains(&"tigertype-linux-ui"));
            assert!(command.contains(&"tigertype-backend"));
            assert!(!command.contains(&"tigertype-windows-app"));
            assert!(!command.contains(&"tigertype-macos-app"));
        }
    }
}
