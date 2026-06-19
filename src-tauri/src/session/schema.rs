//! SQLite schema（P1）。

pub const CREATE_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS terminals (
    id          TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    cmd         TEXT NOT NULL,
    args        TEXT NOT NULL,   -- JSON array
    cwd         TEXT NOT NULL,
    env         TEXT NOT NULL,   -- JSON object
    created_at  TEXT NOT NULL
);
"#;
