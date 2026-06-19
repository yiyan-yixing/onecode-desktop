//! 应用配置（M1 用默认值；M2/M3 可从 ~/.onecode/desktop.json 读取）。

#[derive(Clone)]
pub struct AppConfig {
    /// 默认启动命令
    pub default_cmd: String,
    /// 默认参数
    pub default_args: Vec<String>,
    /// 默认工作目录（未指定 cwd 时）
    pub default_cwd: String,
    /// 最大并发终端数（对齐 PRD P0-2 的 10 上限）
    pub max_terminals: usize,
    /// 每个 slot 的 ring buffer 上限（MB）
    pub ring_buffer_max_mb: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        Self {
            default_cmd: "claude".to_string(),
            default_args: vec![
                "--permission-mode".into(),
                "bypassPermissions".into(),
            ],
            default_cwd: home,
            max_terminals: 10,
            ring_buffer_max_mb: 10,
        }
    }
}
