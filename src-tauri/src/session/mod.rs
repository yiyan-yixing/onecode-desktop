//! SessionStore — SQLite 会话持久化（P1 完整实现）。
//!
//! 设计（见 desktop-prd.md §5.6 / desktop-code-structure.md）：
//! 保存终端**配置**（{id,label,cmd,args,cwd,env}），应用重启后按配置重新 spawn。
//! 与 tmux 不同，**不保存终端输出内容**——PTY 无法真正恢复终端状态。
//!
//! 调用方：`session_persist` 命令（前端 create/close/rename 去抖触发） +
//! `session_restore` 命令（前端启动时读取并重建终端）。

mod schema;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct PersistentSlot {
    pub id: String,
    pub label: String,
    pub project_id: Option<String>,
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: std::collections::HashMap<String, String>,
    pub backend: String,
    /// agent 身份（`@name`）；None = 通用终端。重启后据此恢复身份。
    pub agent: Option<String>,
    /// 绑定的供应商 id（自选模型）；None = 跟随全局激活供应商。
    pub provider_id: Option<String>,
    /// 该槽实际使用的模型。
    pub model: Option<String>,
    pub created_at: String,
    pub last_active_at: String,
}

pub struct SessionStore {
    db: Arc<Mutex<rusqlite::Connection>>,
}

/// 列是否存在。四个 migration 共用一份实现 —— 避免同一段 PRAGMA 探针
/// 抄四遍、改一处漏三处。
fn has_column(conn: &rusqlite::Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    Ok(rows.any(|c| c.map(|c| c == column).unwrap_or(false)))
}

impl SessionStore {
    pub fn new(data_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&data_dir).ok();
        let path = data_dir.join("sessions.db");
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(schema::CREATE_TABLE_SQL)?;
        // Migrations: 旧库补列（升级路径）
        let migrations: &[(&str, &str)] = &[
            ("project_id", "ALTER TABLE terminals ADD COLUMN project_id TEXT"),
            (
                "backend",
                "ALTER TABLE terminals ADD COLUMN backend TEXT NOT NULL DEFAULT 'claude-code'",
            ),
            (
                "last_active_at",
                "ALTER TABLE terminals ADD COLUMN last_active_at TEXT NOT NULL DEFAULT ''",
            ),
            // agent 身份（@name 派发）。可空：旧的通用终端没有身份。
            ("agent", "ALTER TABLE terminals ADD COLUMN agent TEXT"),
            // 每槽路由（自选模型）。可空：旧的槽跟随全局激活供应商。
            ("provider_id", "ALTER TABLE terminals ADD COLUMN provider_id TEXT"),
            ("model", "ALTER TABLE terminals ADD COLUMN model TEXT"),
        ];
        for (column, sql) in migrations {
            if !has_column(&conn, "terminals", column)? {
                conn.execute_batch(sql)?;
                log::info!("[session] migrated: added {column} column");
            }
        }
        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn save_all(&self, slots: &[PersistentSlot]) -> Result<()> {
        let db = self.db.lock().await;
        db.execute_batch("BEGIN")?;
        // 即使后续 INSERT 失败，DELETE 也可回滚
        db.execute("DELETE FROM terminals", [])?;
        for s in slots {
            db.execute(
                "INSERT INTO terminals (id, label, project_id, cmd, args, cwd, env, backend, agent, provider_id, model, created_at, last_active_at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                rusqlite::params![
                    s.id,
                    s.label,
                    s.project_id,
                    s.cmd,
                    serde_json::to_string(&s.args)?,
                    s.cwd,
                    serde_json::to_string(&s.env)?,
                    s.backend,
                    s.agent,
                    s.provider_id,
                    s.model,
                    s.created_at,
                    s.last_active_at,
                ],
            )?;
        }
        db.execute_batch("COMMIT")?;
        Ok(())
    }

    pub async fn load_all(&self) -> Result<Vec<PersistentSlot>> {
        let db = self.db.lock().await;
        let mut stmt = db.prepare(
            "SELECT id, label, project_id, cmd, args, cwd, env, backend, agent, provider_id, model, created_at, last_active_at \
             FROM terminals ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            let args_json: String = row.get(4)?;
            let env_json: String = row.get(6)?;
            Ok(PersistentSlot {
                id: row.get(0)?,
                label: row.get(1)?,
                project_id: row.get(2)?,
                cmd: row.get(3)?,
                args: serde_json::from_str(&args_json).unwrap_or_default(),
                cwd: row.get(5)?,
                env: serde_json::from_str(&env_json).unwrap_or_default(),
                backend: row
                    .get::<_, Option<String>>(7)?
                    .unwrap_or_else(|| "claude-code".to_string()),
                agent: row.get::<_, Option<String>>(8)?,
                provider_id: row.get::<_, Option<String>>(9)?,
                model: row.get::<_, Option<String>>(10)?,
                created_at: row.get(11)?,
                last_active_at: row.get::<_, Option<String>>(12)?.unwrap_or_default(),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(id: &str, agent: Option<&str>) -> PersistentSlot {
        PersistentSlot {
            id: id.to_string(),
            label: "L".to_string(),
            project_id: Some("p1".to_string()),
            cmd: "claude".to_string(),
            args: vec!["--permission-mode".to_string()],
            cwd: "/tmp/p1".to_string(),
            env: std::collections::HashMap::new(),
            backend: "claude-code".to_string(),
            agent: agent.map(|s| s.to_string()),
            provider_id: Some("deepseek".to_string()),
            model: Some("deepseek-chat".to_string()),
            created_at: "2026-10-14T00:00:00Z".to_string(),
            last_active_at: "2026-10-14T01:00:00Z".to_string(),
        }
    }

    /// agent 身份必须能跨重启恢复 —— 否则"@name 真派发"只在当次会话有效，
    /// 重启后员工就退化成通用终端。
    #[test]
    fn agent_identity_survives_roundtrip() {
        let dir = std::env::temp_dir().join(format!("dsh-session-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let store = SessionStore::new(dir.clone()).unwrap();

        let slots = vec![slot("a", Some("architect")), slot("b", None)];
        rt.block_on(store.save_all(&slots)).unwrap();
        let loaded = rt.block_on(store.load_all()).unwrap();

        assert_eq!(loaded.len(), 2);
        let a = loaded.iter().find(|s| s.id == "a").unwrap();
        assert_eq!(a.agent.as_deref(), Some("architect"));
        assert_eq!(a.provider_id.as_deref(), Some("deepseek"), "每槽路由必须能跨重启恢复");
        assert_eq!(a.model.as_deref(), Some("deepseek-chat"));
        let b = loaded.iter().find(|s| s.id == "b").unwrap();
        assert_eq!(b.agent, None, "通用终端必须保持 None，不能被写成空串");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 旧库（无 agent 列）升级后必须可用，且既有行读出来是 None。
    #[test]
    fn migration_adds_agent_column_to_legacy_db() {
        let dir = std::env::temp_dir().join(format!("dsh-session-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 造一个没有 agent 列的旧表
        {
            let conn = rusqlite::Connection::open(dir.join("sessions.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE terminals (
                    id TEXT PRIMARY KEY, label TEXT NOT NULL, project_id TEXT,
                    cmd TEXT NOT NULL, args TEXT NOT NULL, cwd TEXT NOT NULL,
                    env TEXT NOT NULL, backend TEXT NOT NULL DEFAULT 'claude-code',
                    created_at TEXT NOT NULL, last_active_at TEXT NOT NULL DEFAULT '');
                 INSERT INTO terminals (id,label,cmd,args,cwd,env,created_at)
                 VALUES ('old','L','claude','[]','/tmp','{}','t');",
            )
            .unwrap();
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        let store = SessionStore::new(dir.clone()).unwrap();
        assert!(has_column(&store.db.blocking_lock(), "terminals", "agent").unwrap());
        let loaded = rt.block_on(store.load_all()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].agent, None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
