//! SQLite schema（P1）。

pub const CREATE_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS terminals (
    id              TEXT PRIMARY KEY,
    label           TEXT NOT NULL,
    project_id      TEXT,
    cmd             TEXT NOT NULL,
    args            TEXT NOT NULL,   -- JSON array
    cwd             TEXT NOT NULL,
    env             TEXT NOT NULL,   -- JSON object
    backend         TEXT NOT NULL DEFAULT 'claude-code',
    agent           TEXT,            -- agent 身份（@name）；NULL = 通用终端
    provider_id     TEXT,            -- 绑定的供应商 id；NULL = 跟随全局激活
    model           TEXT,            -- 该槽实际使用的模型
    created_at      TEXT NOT NULL,
    last_active_at  TEXT NOT NULL DEFAULT ''
);
"#;
