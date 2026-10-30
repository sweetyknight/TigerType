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
    "tigertype-dev",
];
fn selection(macos: bool) -> Vec<&'static str> {
    if macos {
        MAC.iter().flat_map(|package| ["-p", *package]).collect()
    } else {
        vec!["--workspace"]
    }
}
pub fn commands(macos: bool) -> Vec<Vec<&'static str>> {
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
    let mut lint = vec!["clippy", "--locked", "--offline", "--all-targets"];
    lint.extend(selection(macos));
    lint.extend(["--", "-D", "warnings"]);
    commands.push(lint);
    // 明确测试种类，避免 Windows 重链接仍在运行的 check.exe；all-targets Clippy 仍覆盖示例。
    let native_targets: &[Option<&str>] = if macos {
        &[None]
    } else {
        &[None, Some("i686-pc-windows-msvc")]
    };
    for target in native_targets {
        for kinds in [&["--lib", "--bins", "--tests"][..], &["--doc"][..]] {
            let mut test = vec!["test", "--locked", "--offline"];
            test.extend(selection(macos));
            test.extend(kinds);
            if let Some(target) = target {
                test.extend(["--target", target]);
            }
            commands.push(test);
        }
    }
    let targets = if macos {
        ["x86_64-pc-windows-msvc", "i686-pc-windows-msvc"]
    } else {
        ["aarch64-apple-darwin", "x86_64-apple-darwin"]
    };
    for target in targets {
        let mut lint = vec!["clippy", "--locked", "--offline", "--all-targets"];
        lint.extend(selection(!macos));
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
        let win = commands(false);
        let mac = commands(true);
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
    }
}
