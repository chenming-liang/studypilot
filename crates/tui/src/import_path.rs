//! 导入路径解析与校验（文档 Agent项目改进建议.md 四/十三/十四）。
//!
//! 目标：从"任意可访问的本地位置"导入——绝对路径按原样使用，相对路径相对 cwd 解析，
//! 不做 project-root 限制、不手写字符串判断 Windows/Linux 路径（统一用 `std::path`）。
//! 所有入口（CLI / Wizard / Palette / Course 页）最终都走同一个 resolver + 同一个 run_import。

use std::path::{Path, PathBuf};

/// 解析后的导入目标。
#[derive(Debug, Clone)]
pub struct ImportTarget {
    pub path: PathBuf,
    /// 是文件还是目录（决定错误提示与扫尾语义；核心 walkdir 对两者都兼容）
    pub is_file: bool,
}

/// 用户可读的导入路径错误（不是 Rust debug error）。
#[derive(Debug)]
pub enum ImportPathError {
    /// 路径不存在（区分 not found）
    NotFound(PathBuf),
    /// 既不是文件也不是目录（socket 等特殊类型）
    NotFileOrDir(PathBuf),
    /// 单文件但扩展名不受支持
    UnsupportedType(PathBuf),
    /// 目录下没有任何受支持的文件
    EmptyDir(PathBuf),
    /// 存在但无法访问（权限等）
    Inaccessible(PathBuf, std::io::Error),
}

impl std::fmt::Display for ImportPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportPathError::NotFound(p) => write!(
                f,
                "File not found: {}\nCheck the path and try again.",
                p.display()
            ),
            ImportPathError::NotFileOrDir(p) => {
                write!(f, "Not a file or directory: {}", p.display())
            }
            ImportPathError::UnsupportedType(p) => write!(
                f,
                "Unsupported file type: {}\nSupported: md / txt / pdf / pptx",
                p.display()
            ),
            ImportPathError::EmptyDir(p) => write!(
                f,
                "No supported files found in: {}\nSupported: md / txt / pdf / pptx",
                p.display()
            ),
            ImportPathError::Inaccessible(p, e) => {
                write!(f, "Could not access: {}\nReason: {}", p.display(), e)
            }
        }
    }
}

/// 受支持的文件扩展名（与 importer::pipeline::collect_files 同一口径）。
pub(crate) fn is_supported_file(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .map(|x| {
            matches!(
                x.to_ascii_lowercase().as_str(),
                "md" | "txt" | "pdf" | "pptx"
            )
        })
        .unwrap_or(false)
}

/// 目录下是否有任何受支持的文件（递归；与 importer::collect_files 同口径的简化探测）。
fn dir_has_supported_files(dir: &Path) -> bool {
    fn walk(p: &Path) -> bool {
        let Ok(rd) = std::fs::read_dir(p) else {
            return false;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if walk(&path) {
                    return true;
                }
            } else if is_supported_file(&path) {
                return true;
            }
        }
        false
    }
    walk(dir)
}

/// 统一的导入路径解析：
/// - trim + 剥用户输入引号
/// - `~`（前缀）→ `$HOME` 展开（用户习惯的绝对路径写法）
/// - `PathBuf::from`；绝对路径原样使用，相对路径相对 current_dir() 解析
/// - 校验：存在 → 文件/目录 → 支持类型/空目录
///
/// 不做 project-root 限制，不做跨平台字符串判断。
pub(crate) fn resolve_import_path(input: &str) -> Result<ImportTarget, ImportPathError> {
    let trimmed = input.trim().trim_matches('"').trim();
    if trimmed.is_empty() {
        return Err(ImportPathError::NotFound(PathBuf::new()));
    }
    // `~` 前缀展开为 HOME（仅裸 `~` 或 `~/...`；普通相对名/绝对路径不受影响）
    let expanded = if trimmed == "~" {
        match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            Some(h) => PathBuf::from(h),
            None => PathBuf::from(trimmed),
        }
    } else if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            Some(h) => PathBuf::from(h).join(rest),
            None => PathBuf::from(trimmed),
        }
    } else {
        PathBuf::from(trimmed)
    };
    let raw = expanded;
    let path = if raw.is_absolute() {
        raw
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(raw),
            Err(e) => return Err(ImportPathError::Inaccessible(raw, e)),
        }
    };

    let meta = std::fs::metadata(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ImportPathError::NotFound(path.clone()),
        _ => ImportPathError::Inaccessible(path.clone(), e),
    })?;

    // 归一化路径（展开 ../、./，且保证绝对路径可被后续存取）——
    // 仅在确认存在后 canonicalize，不存在时留 error 归因给用户。
    let canon = std::fs::canonicalize(&path).unwrap_or(path.clone());

    if meta.is_file() {
        if !is_supported_file(&canon) {
            return Err(ImportPathError::UnsupportedType(canon));
        }
        Ok(ImportTarget {
            path: canon,
            is_file: true,
        })
    } else if meta.is_dir() {
        if !dir_has_supported_files(&canon) {
            return Err(ImportPathError::EmptyDir(canon));
        }
        Ok(ImportTarget {
            path: canon,
            is_file: false,
        })
    } else {
        Err(ImportPathError::NotFileOrDir(canon))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 相对路径测试会改 cwd，串行化避免并行干扰。
    static CWD_LOCK: Mutex<()> = Mutex::new(());

    /// 临时目录：{root}/materials/notes.md + {root}/materials/sub/ppt.pptx
    fn make_tree(root: &Path) {
        std::fs::create_dir_all(root.join("materials/sub")).unwrap();
        std::fs::write(root.join("materials/notes.md"), "# hi").unwrap();
        std::fs::write(root.join("materials/sub/slides.pptx"), "ppt").unwrap();
        std::fs::write(root.join("materials/ignore.txt"), "x").unwrap();
        std::fs::write(root.join("solo.pdf"), "pdf").unwrap();
        std::fs::write(root.join("bogus.exe"), "nope").unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
    }

    fn tmp_root(tag: &str) -> PathBuf {
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("sp-import-{tag}-{}-{ns}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        make_tree(&dir);
        dir
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn absolute_dir_resolves() {
        let root = tmp_root("absdir");
        let t = resolve_import_path(&root.join("materials").display().to_string()).unwrap();
        assert!(!t.is_file);
        assert_eq!(t.path, root.join("materials"));
        cleanup(&root);
    }

    #[test]
    fn absolute_single_file_resolves() {
        let root = tmp_root("absfile");
        let t = resolve_import_path(&root.join("solo.pdf").display().to_string()).unwrap();
        assert!(t.is_file);
        cleanup(&root);
    }

    #[test]
    fn path_with_spaces_resolves() {
        let root = tmp_root("spaces");
        let spaced = root.join("my materials");
        std::fs::create_dir_all(&spaced).unwrap();
        std::fs::write(spaced.join("a.md"), "x").unwrap();
        let t = resolve_import_path(&spaced.display().to_string()).unwrap();
        assert!(!t.is_file);
        cleanup(&root);
    }

    #[test]
    fn relative_dir_resolves_against_cwd() {
        let _g = CWD_LOCK.lock().unwrap();
        let root = tmp_root("reldir");
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        let t = resolve_import_path("./materials").unwrap();
        assert_eq!(t.path, root.join("materials"));
        std::env::set_current_dir(&cwd).unwrap();
        cleanup(&root);
    }

    #[test]
    fn parent_relative_dir_resolves() {
        let _g = CWD_LOCK.lock().unwrap();
        let root = tmp_root("parrel");
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(root.join("materials")).unwrap();
        let t = resolve_import_path("../materials").unwrap();
        assert_eq!(t.path, root.join("materials"));
        std::env::set_current_dir(&cwd).unwrap();
        cleanup(&root);
    }

    #[test]
    fn bare_tokens_resolve_as_relative() {
        let _g = CWD_LOCK.lock().unwrap();
        let root = tmp_root("bare");
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        let t = resolve_import_path("materials").unwrap();
        assert_eq!(t.path, root.join("materials"));
        std::env::set_current_dir(&cwd).unwrap();
        cleanup(&root);
    }

    #[test]
    fn quotes_are_stripped() {
        let root = tmp_root("quotes");
        let q = format!("\"{}\"", root.join("materials").display());
        let t = resolve_import_path(&q).unwrap();
        assert_eq!(t.path, root.join("materials"));
        cleanup(&root);
    }

    #[test]
    fn missing_path_is_not_found() {
        let root = tmp_root("missing");
        match resolve_import_path(&root.join("nope").display().to_string()) {
            Err(ImportPathError::NotFound(_)) => {}
            other => panic!("应为 NotFound，实际 {other:?}"),
        }
        cleanup(&root);
    }

    #[test]
    fn unsupported_single_file_rejected() {
        let root = tmp_root("badtype");
        match resolve_import_path(&root.join("bogus.exe").display().to_string()) {
            Err(ImportPathError::UnsupportedType(_)) => {}
            other => panic!("应为 UnsupportedType，实际 {other:?}"),
        }
        cleanup(&root);
    }

    #[test]
    fn empty_dir_rejected() {
        let root = tmp_root("emptydir");
        match resolve_import_path(&root.join("empty").display().to_string()) {
            Err(ImportPathError::EmptyDir(_)) => {}
            other => panic!("应为 EmptyDir，实际 {other:?}"),
        }
        cleanup(&root);
    }

    #[test]
    fn blank_input_is_not_found() {
        assert!(matches!(
            resolve_import_path("  "),
            Err(ImportPathError::NotFound(_))
        ));
    }

    /// `~` 前缀展开为 HOME（用户常见的绝对路径写法；旧 bug：被当相对路径拼 cwd）。
    #[test]
    fn tilde_prefix_expands_to_home() {
        static HOME_LOCK: Mutex<()> = Mutex::new(());
        let _g = HOME_LOCK.lock().unwrap();
        let root = tmp_root("tilde");
        let orig = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", &root);
        }
        let t = resolve_import_path("~/materials").unwrap();
        assert_eq!(t.path, root.join("materials"), "~/x 应展开为 $HOME/x");
        let t2 = resolve_import_path("~").unwrap();
        assert_eq!(t2.path, root, "裸 ~ 应展开为 $HOME");
        match orig {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        cleanup(&root);
    }

    /// 真实绝对路径（用户语料 ~/rust/test-agent）：验证跨 HOME 的绝对目录导入。
    /// `#[ignore]`：依赖本机目录，跑 `cargo test -p tui real_absolute -- --ignored`。
    #[test]
    #[ignore = "cargo test -p tui real_absolute -- --ignored（依赖 ~/rust/test-agent）"]
    fn real_absolute_dir_resolves() {
        let home = std::env::var("HOME").expect("无 HOME");
        let dir = std::path::PathBuf::from(&home).join("rust/test-agent/resources");
        assert!(dir.is_dir(), "语料目录应存在: {}", dir.display());

        let t = resolve_import_path(&dir.display().to_string()).expect("绝对目录应解析成功");
        assert!(!t.is_file, "resources 是目录");
        assert_eq!(t.path, std::fs::canonicalize(&dir).unwrap());

        // Windows 下载副产物（:Zone.Identifier 无扩展名）应被扩展名过滤忽略；
        // 目录必须仍能发现受支持的 pdf。
        assert!(dir_has_supported_files(&dir), "目录应包含受支持文件（pdf）");
        println!("绝对路径 OK: {}", t.path.display());
    }
}
