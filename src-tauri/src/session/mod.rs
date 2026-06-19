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
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: std::collections::HashMap<String, String>,
    pub created_at: String,
}

pub struct SessionStore {
    db: Arc<Mutex<rusqlite::Connection>>,
}

impl SessionStore {
    pub fn new(data_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&data_dir).ok();
        let path = data_dir.join("sessions.db");
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(schema::CREATE_TABLE_SQL)?;
        Ok(Self {
            db: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn save_all(&self, slots: &[PersistentSlot]) -> Result<()> {
        let db = self.db.lock().await;
        db.execute("DELETE FROM terminals", [])?;
        for s in slots {
            db.execute(
                "INSERT INTO terminals (id, label, cmd, args, cwd, env, created_at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                rusqlite::params![
                    s.id,
                    s.label,
                    s.cmd,
                    serde_json::to_string(&s.args)?,
                    s.cwd,
                    serde_json::to_string(&s.env)?,
                    s.created_at,
                ],
            )?;
        }
        Ok(())
    }

    pub async fn load_all(&self) -> Result<Vec<PersistentSlot>> {
        let db = self.db.lock().await;
        let mut stmt = db.prepare(
            "SELECT id, label, cmd, args, cwd, env, created_at \
             FROM terminals ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            let args_json: String = row.get(3)?;
            let env_json: String = row.get(5)?;
            Ok(PersistentSlot {
                id: row.get(0)?,
                label: row.get(1)?,
                cmd: row.get(2)?,
                args: serde_json::from_str(&args_json).unwrap_or_default(),
                cwd: row.get(4)?,
                env: serde_json::from_str(&env_json).unwrap_or_default(),
                created_at: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}
