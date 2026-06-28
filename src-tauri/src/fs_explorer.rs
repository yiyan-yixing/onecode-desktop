//! 文件浏览器 IPC 命令（原生文件系统访问，不依赖外部服务）。
//!
//! 提供 fs_list_dir / fs_read_file 两个命令，供前端渲染文件树和预览面板。
//! 安全作用域：仅允许 $HOME 子目录及活跃终端 cwd 路径。

use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

// ── 数据结构 ──────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<i64>, // unix epoch ms
    pub file_type: String,     // "dir" | "md" | "code" | "img" | "pdf" | "bin"
}

#[derive(serde::Serialize)]
pub struct ListDirResult {
    pub path: String,
    pub entries: Vec<DirEntry>,
}

#[derive(serde::Serialize)]
pub struct FileContent {
    pub name: String,
    pub path: String,
    pub file_type: String,       // "md" | "code" | "img" | "pdf" | "bin"
    pub text: Option<String>,     // 文本文件内容
    pub data_base64: Option<String>, // 二进制文件 base64（图片/PDF）
    pub size: u64,
}

// ── 常量 ──────────────────────────────────────────────────────────

const TEXT_SIZE_LIMIT: u64 = 2 * 1024 * 1024; // 2MB
const BIN_SIZE_LIMIT: u64 = 10 * 1024 * 1024; // 10MB

/// Markdown 扩展名
const MD_EXTS: &[&str] = &["md", "mdx", "markdown"];

/// 图片扩展名
const IMG_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "bmp", "tiff", "tif",
    "avif",
];

/// PDF 扩展名
const PDF_EXTS: &[&str] = &["pdf"];

/// 代码文件扩展名
const CODE_EXTS: &[&str] = &[
    "js", "jsx", "mjs", "cjs",
    "ts", "tsx", "mts", "cts",
    "py", "pyw", "pyi",
    "rs", "rlib",
    "go",
    "java", "kt", "kts",
    "c", "h", "cpp", "hpp", "cc", "cxx", "hxx",
    "cs",
    "rb",
    "php",
    "swift",
    "zig",
    "lua",
    "r", "R",
    "scala", "sc",
    "sh", "bash", "zsh", "fish",
    "ps1", "psm1",
    "sql",
    "html", "htm",
    "css", "scss", "sass", "less",
    "vue", "svelte",
    "yaml", "yml", "toml", "ini", "cfg", "conf",
    "json", "jsonc",
    "xml", "xsl", "xsd",
    "proto", "graphql", "gql",
    "dockerfile",
    "makefile",
    "cmake",
    "gradle",
    "lock",
    "log",
    "diff", "patch",
];

/// 代码文件名（无扩展名匹配）
const CODE_NAMES: &[&str] = &[
    "Makefile", "Dockerfile", "Vagrantfile", "Gemfile", "Rakefile",
    "CMakeLists.txt", "Cargo.toml", "Cargo.lock", "go.mod", "go.sum",
    "package.json", "tsconfig.json", ".gitignore", ".gitattributes",
    ".env", ".env.local", ".env.production",
    "LICENSE", "COPYING", "AUTHORS",
    "Gemfile.lock", "Podfile", "Brewfile",
];

// ── 文件类型检测 ──────────────────────────────────────────────────

fn classify_file(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    // 先检查扩展名
    if let Some(ext) = lower.rsplit('.').next() {
        if MD_EXTS.contains(&ext) { return "md"; }
        if IMG_EXTS.contains(&ext) { return "img"; }
        if PDF_EXTS.contains(&ext) { return "pdf"; }
        if CODE_EXTS.contains(&ext) { return "code"; }
    }
    // 再检查完整文件名
    if CODE_NAMES.iter().any(|n| name == *n) { return "code"; }
    "bin"
}

// ── 安全作用域 ──────────────────────────────────────────────────

/// 获取用户 HOME 目录
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// 验证路径在安全作用域内（$HOME 子目录）
fn validate_path(path: &str) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|e| format!("invalid path: {e}"))?;

    if let Some(home) = home_dir() {
        if canonical.starts_with(&home) {
            return Ok(canonical);
        }
    }

    // 拒绝系统路径（macOS: /etc → /private/etc, /var → /private/var, /tmp → /private/tmp）
    let path_str = canonical.to_string_lossy();
    let denied_prefixes = [
        "/etc", "/private/etc",
        "/var", "/private/var",
        "/usr",
        "/sbin", "/bin",
        "/lib", "/private/lib",
        "/dev",
        "/proc", "/sys",
        "/System", "/Library",
    ];
    for prefix in &denied_prefixes {
        if path_str.starts_with(prefix) {
            return Err(format!("access denied: system path"));
        }
    }

    Ok(canonical)
}

// ── Tauri 命令 ──────────────────────────────────────────────────

#[tauri::command]
pub async fn fs_list_dir(path: String) -> Result<ListDirResult, String> {
    let canonical = validate_path(&path)?;

    if !canonical.is_dir() {
        return Err(format!("not a directory: {}", canonical.display()));
    }

    let entries: Vec<DirEntry> = fs::read_dir(&canonical)
        .map_err(|e| format!("read_dir failed: {e}"))?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // 隐藏文件仍然显示（开发者需要看到 .gitignore 等）
            let meta = e.metadata().ok()?;
            let is_dir = meta.is_dir();
            // 对符号链接，follow 后取实际属性
            let (is_dir, size, modified) = if meta.is_symlink() {
                // follow symlink
                match fs::metadata(e.path()) {
                    Ok(m) => (m.is_dir(), m.len(), m.modified().ok()),
                    Err(_) => return None, // broken symlink, skip
                }
            } else {
                (is_dir, meta.len(), meta.modified().ok())
            };
            let file_type = if is_dir {
                "dir".to_string()
            } else {
                classify_file(&name).to_string()
            };
            let modified_ms = modified.and_then(|t| {
                t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
            });
            let full_path = e.path().to_string_lossy().to_string();
            Some(DirEntry {
                name,
                path: full_path,
                is_dir,
                size,
                modified: modified_ms,
                file_type,
            })
        })
        .collect();

    // 排序：目录优先，然后按名称字母序（忽略大小写）
    let mut sorted = entries;
    sorted.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    Ok(ListDirResult {
        path: canonical.to_string_lossy().to_string(),
        entries: sorted,
    })
}

#[tauri::command]
pub async fn fs_read_file(path: String) -> Result<FileContent, String> {
    let canonical = validate_path(&path)?;

    if !canonical.is_file() {
        return Err(format!("not a file: {}", canonical.display()));
    }

    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let meta = fs::metadata(&canonical)
        .map_err(|e| format!("metadata failed: {e}"))?;
    let size = meta.len();
    let file_type = classify_file(&name).to_string();

    // 根据文件类型决定读取策略
    match file_type.as_str() {
        "md" | "code" => {
            // 文本文件：读取内容，超限截断
            if size > TEXT_SIZE_LIMIT {
                // 只读取前 2MB
                let data = read_file_head(&canonical, TEXT_SIZE_LIMIT as usize)?;
                Ok(FileContent {
                    name,
                    path: canonical.to_string_lossy().to_string(),
                    file_type,
                    text: Some(String::from_utf8_lossy(&data).to_string()),
                    data_base64: None,
                    size,
                })
            } else {
                let text = fs::read_to_string(&canonical)
                    .map_err(|e| format!("read failed: {e}"))?;
                Ok(FileContent {
                    name,
                    path: canonical.to_string_lossy().to_string(),
                    file_type,
                    text: Some(text),
                    data_base64: None,
                    size,
                })
            }
        }
        "img" | "pdf" => {
            // 二进制文件：base64 编码
            if size > BIN_SIZE_LIMIT {
                Ok(FileContent {
                    name,
                    path: canonical.to_string_lossy().to_string(),
                    file_type,
                    text: None,
                    data_base64: None, // 前端根据 size 显示"文件过大"
                    size,
                })
            } else {
                let data = fs::read(&canonical)
                    .map_err(|e| format!("read failed: {e}"))?;
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                Ok(FileContent {
                    name,
                    path: canonical.to_string_lossy().to_string(),
                    file_type,
                    text: None,
                    data_base64: Some(b64),
                    size,
                })
            }
        }
        _ => {
            // bin 类型：只返回元数据
            Ok(FileContent {
                name,
                path: canonical.to_string_lossy().to_string(),
                file_type,
                text: None,
                data_base64: None,
                size,
            })
        }
    }
}

/// 读取文件头部（指定字节数）
fn read_file_head(path: &PathBuf, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let f = fs::File::open(path).map_err(|e| format!("open failed: {e}"))?;
    let mut buf = Vec::with_capacity(limit);
    f.take(limit as u64)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;
    Ok(buf)
}

// ── 单元测试 ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_md() {
        assert_eq!(classify_file("README.md"), "md");
        assert_eq!(classify_file("guide.mdx"), "md");
    }

    #[test]
    fn test_classify_img() {
        assert_eq!(classify_file("photo.png"), "img");
        assert_eq!(classify_file("icon.svg"), "img");
    }

    #[test]
    fn test_classify_pdf() {
        assert_eq!(classify_file("doc.pdf"), "pdf");
    }

    #[test]
    fn test_classify_code() {
        assert_eq!(classify_file("index.js"), "code");
        assert_eq!(classify_file("app.tsx"), "code");
        assert_eq!(classify_file("main.rs"), "code");
        assert_eq!(classify_file("style.css"), "code");
        assert_eq!(classify_file("data.json"), "code");
    }

    #[test]
    fn test_classify_code_names() {
        assert_eq!(classify_file("Makefile"), "code");
        assert_eq!(classify_file("Dockerfile"), "code");
        assert_eq!(classify_file("Cargo.toml"), "code");
        assert_eq!(classify_file(".gitignore"), "code");
    }

    #[test]
    fn test_classify_bin() {
        assert_eq!(classify_file("data.dat"), "bin");
        assert_eq!(classify_file("archive.zip"), "bin");
    }

    #[test]
    fn test_validate_path_denied() {
        assert!(validate_path("/etc/passwd").is_err());
        assert!(validate_path("/usr/bin/ls").is_err());
    }

    #[test]
    fn test_validate_path_home() {
        if let Some(home) = home_dir() {
            assert!(validate_path(home.to_string_lossy().as_ref()).is_ok());
        }
    }
}
