//! Tauri invoke 命令层（前端 → Rust）。
//!
//! 架构修订（review §2）：数据传输用 Channel<Vec<u8>>，不再 base64。
//! - pty_spawn / pty_restart 接收 `data_channel: Channel<Vec<u8>>`（流式二进制）。
//! - pty_write 的 data 是 `Vec<u8>`（零编码）。
//! - pty_replay 返回 `Vec<u8>`（Tab 切换回放）。

use std::collections::HashMap;
use std::path::PathBuf;

use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};

use crate::cc_sessions::CcSessionsCache;
use crate::cc_status::CcStatusCache;
use crate::config::{AppConfig, ConfigManager, ConfigUpdate};
use crate::providers::{self, ProviderInput, ProviderStore, ProviderUpdate};
use crate::pty::health::{check_health, HealthReport};
use crate::pty::{MultiPtyManager, SlotConfig, SlotSummary};
use crate::session::{PersistentSlot, SessionStore};

#[derive(serde::Serialize)]
pub struct SpawnResult {
    pub id: String,
    pub pid: Option<u32>,
    /// agent 身份是否**真的**注入了。
    /// 后端未声明 `system_prompt_file_flag`、或 agent 定义找不到、或写文件失败时为
    /// `false` —— 前端据此如实显示"通用终端"，不假装它有身份。
    pub agent: Option<String>,
    pub agent_injected: bool,
    /// 实际生效的 provider id（请求的 provider 不在目录里时会与请求值不同）
    pub effective_provider: Option<String>,
    /// 实际生效的模型
    pub effective_model: Option<String>,
    /// 请求了 provider 但没找到 → 已回退到全局。前端必须如实显示。
    pub provider_fallback: bool,
    /// 这条路线怎么定下来的：requested | roster | global
    pub route_source: String,
    /// failover 换过路线时的原 provider（如实显示，别让人以为用的是自己设的那个）
    pub switched_from: Option<String>,
    /// 换路线/没换成的原因
    pub switch_reason: Option<String>,
    /// 实际生效的权限档（`acceptEdits` 等）。None = 不干预（沿用后端默认）。
    pub posture: Option<String>,
    /// 权限档是**推断**的（员工只声明了 `tools:`、没声明 `permission:`）——
    /// 界面必须标明，否则用户会以为是自己设的。
    pub posture_inferred: bool,
    /// 实际生效的工具白名单
    pub allowed_tools: Vec<String>,
    /// 权限相关的**未生效原因**（未识别取值 / 后端不支持）。前端要显示。
    pub posture_note: Option<String>,
    /// 已注入的技能名
    pub skills_loaded: Vec<String>,
    /// 在约定根目录里找不到的技能名
    pub skills_missing: Vec<String>,
    /// 找到了但正文超预算、改为「给路径让模型按需读」的技能名
    pub skills_deferred: Vec<String>,
}

fn parse_id(s: &str) -> Result<uuid::Uuid, String> {
    uuid::Uuid::parse_str(s).map_err(|e| format!("invalid id: {e}"))
}


#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pty_spawn(
    backend: Option<String>,
    cmd: Option<String>,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    label: Option<String>,
    project_id: Option<String>,
    // agent 身份（`@name`）：以该 agent 为身份启动，而不是往同一个 PTY 打文本。
    agent: Option<String>,
    // 每槽路由（自选模型）：指定 provider_id → 用该供应商的凭据，而不是全局激活的那个。
    provider_id: Option<String>,
    // 模型覆盖：非空则覆盖该 provider 的默认模型（允许同一供应商换模型）。
    model: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
    data_channel: Channel<Vec<u8>>,
    state: State<'_, MultiPtyManager>,
    cfg_mgr: State<'_, ConfigManager>,
    providers: State<'_, ProviderStore>,
) -> Result<SpawnResult, String> {
    // 从 ConfigManager 读当前配置快照（M1 切档后新 spawn 读到新 creds；修复 P0-1：
    // 托管 Arc<AppConfig> 在 Tauri 2 无法覆盖，必须读可变 RwLock 而非冻结快照）
    let cfg_arc = cfg_mgr.arc();
    let config = cfg_arc.read().await;

    // ★ 计划生成整段收敛到 `spawn_plan::plan`（**纯函数**）。
    // 这段以前住在这里的函数体里，而它汇合了身份注入 / 权限姿态 / 技能组合 /
    // 每槽路由 / failover —— 全产品最核心的一条链路，却因为依赖 `State<...>`
    // 一个测试都没有。抽出去之后既能单测，也能真的拿去起一个桩引擎验证参数到达。
    let agent_for_result = agent.clone();
    let req = crate::spawn_plan::SpawnRequest {
        backend,
        cmd,
        args,
        cwd,
        env,
        agent,
        provider_id,
        model,
    };
    let plan = {
        let catalog = providers.arc();
        let mut catalog = catalog.write().await;
        let plan = crate::spawn_plan::plan(&req, &config, &catalog);
        // 真的换了路线就记一条历史 —— `history` 与 `failover_queue` 这两个字段
        // 从 M1 起就落盘了但一直没人写，这里把它接上（否则那个字段就是死的）。
        // 纯函数只把要记的东西交回来，改 catalog 由调用方做。
        if let Some((from, to, reason)) = &plan.switch_record {
            providers::record_switch(&mut catalog, from, to, reason);
        }
        plan
    };
    for w in &plan.warnings {
        log::warn!("[pty_spawn] {w}");
    }

    log::info!(
        "[pty_spawn] backend={} cmd={:?} args={:?} cwd={:?} project_id={:?} agent={:?} injected={} \
         provider={:?} model={:?} route_source={} fallback={} cols={cols:?} rows={rows:?}",
        plan.backend,
        plan.cmd,
        plan.args,
        plan.cwd,
        project_id,
        agent_for_result,
        plan.agent_injected,
        plan.provider_id,
        plan.model,
        plan.route_source,
        plan.provider_fallback
    );
    let (id, pid) = state
        .spawn(
            plan.cmd.clone(),
            plan.args.clone(),
            plan.cwd.clone(),
            plan.env.clone(),
            data_channel,
            label,
            project_id,
            plan.backend.clone(),
            agent_for_result.clone(),
            plan.provider_id.clone(),
            Some(plan.model.clone()),
            cols,
            rows,
        )
        .await
        .map_err(|e| {
            log::error!("[pty_spawn] FAILED: {e}");
            e.to_string()
        })?;
    log::info!(
        "[pty_spawn] OK id={id} pid={pid:?} agent={:?} injected={} provider={:?}",
        plan.agent,
        plan.agent_injected,
        plan.provider_id
    );
    Ok(SpawnResult {
        id: id.to_string(),
        pid,
        agent: agent_for_result,
        agent_injected: plan.agent_injected,
        effective_provider: plan.provider_id,
        effective_model: Some(plan.model),
        provider_fallback: plan.provider_fallback,
        route_source: plan.route_source,
        switched_from: plan.switched_from,
        switch_reason: plan.switch_reason,
        posture: plan.posture,
        posture_inferred: plan.posture_inferred,
        allowed_tools: plan.allowed_tools,
        posture_note: plan.posture_note,
        skills_loaded: plan.skills_loaded,
        skills_missing: plan.skills_missing,
        skills_deferred: plan.skills_deferred,
    })
}

#[tauri::command]
pub async fn pty_kill(id: String, state: State<'_, MultiPtyManager>) -> Result<(), String> {
    state.kill(parse_id(&id)?).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pty_restart(
    id: String,
    cols: Option<u16>,
    rows: Option<u16>,
    data_channel: Channel<Vec<u8>>,
    state: State<'_, MultiPtyManager>,
) -> Result<(), String> {
    state
        .restart(parse_id(&id)?, data_channel, cols, rows)
        .await
        .map_err(|e| e.to_string())
}

/// 前端键盘输入 → PTY。data 为原始字节（UTF-8）。
#[tauri::command]
pub async fn pty_write(
    id: String,
    data: Vec<u8>,
    state: State<'_, MultiPtyManager>,
) -> Result<(), String> {
    state
        .write(parse_id(&id)?, data)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pty_resize(
    id: String,
    cols: u16,
    rows: u16,
    state: State<'_, MultiPtyManager>,
) -> Result<(), String> {
    state
        .resize(parse_id(&id)?, cols, rows)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pty_list(state: State<'_, MultiPtyManager>) -> Result<Vec<SlotSummary>, String> {
    Ok(state.list().await)
}

#[tauri::command]
pub async fn pty_rename(
    id: String,
    label: String,
    state: State<'_, MultiPtyManager>,
) -> Result<(), String> {
    state
        .rename(parse_id(&id)?, label)
        .await
        .map_err(|e| e.to_string())
}

/// Tab 切换时前端拉取 ring buffer 回放（返回原始字节）。
#[tauri::command]
pub async fn pty_replay(id: String, state: State<'_, MultiPtyManager>) -> Result<Vec<u8>, String> {
    state
        .replay(parse_id(&id)?)
        .await
        .map_err(|e| e.to_string())
}

/// 标记指定终端为当前活跃（前端 switchTo 时调用）。
/// 更新 last_active_at 时间戳，供 session 恢复时定位上次使用的终端。
#[tauri::command]
pub async fn pty_set_active(id: String, state: State<'_, MultiPtyManager>) -> Result<(), String> {
    state
        .set_active(parse_id(&id)?)
        .await
        .map_err(|e| e.to_string())
}

// ── P1：会话持久化 ──────────────────────────────────────────────────

#[tauri::command]
pub async fn session_save(
    slots: Vec<PersistentSlot>,
    state: State<'_, SessionStore>,
) -> Result<(), String> {
    state.save_all(&slots).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn session_restore(
    state: State<'_, SessionStore>,
) -> Result<Vec<PersistentSlot>, String> {
    state.load_all().await.map_err(|e| e.to_string())
}

/// 快照当前所有终端配置并写入 SQLite（前端 create/close/rename 时去抖调用）。
/// 无需前端回传配置——以 Rust 侧 slot 为单一事实源。
#[tauri::command]
pub async fn session_persist(
    pty: State<'_, MultiPtyManager>,
    store: State<'_, SessionStore>,
) -> Result<usize, String> {
    let cfgs: Vec<SlotConfig> = pty.snapshot().await;
    let slots: Vec<PersistentSlot> = cfgs
        .into_iter()
        .map(|c| PersistentSlot {
            id: c.id,
            label: c.label,
            project_id: c.project_id,
            cmd: c.cmd,
            args: c.args,
            cwd: c.cwd,
            env: c.env,
            backend: c.backend,
            agent: c.agent,
            provider_id: c.provider_id,
            model: c.model,
            created_at: c.created_at,
            last_active_at: c.last_active_at,
        })
        .collect();
    let n = slots.len();
    store.save_all(&slots).await.map_err(|e| e.to_string())?;
    Ok(n)
}

// ── P1：CC Status（skills/hooks/plugins/tasks/agents） ──────────────

#[tauri::command]
pub async fn cc_status(
    project_dir: Option<String>,
    state: State<'_, CcStatusCache>,
) -> Result<crate::cc_status::CcStatus, String> {
    let p = project_dir.map(PathBuf::from);
    Ok(state.load(p.as_deref()))
}

/// 清空 CC Status 缓存（前端「刷新」用）。
#[tauri::command]
pub async fn cc_status_invalidate(state: State<'_, CcStatusCache>) -> Result<(), String> {
    state.invalidate();
    Ok(())
}

// ── P1：健康检测 ────────────────────────────────────────────────────

#[tauri::command]
pub async fn health_check(state: State<'_, MultiPtyManager>) -> Result<Vec<HealthReport>, String> {
    let summaries = state.list().await;
    Ok(check_health(&summaries))
}

// ── 配置管理 ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn save_config(
    config: ConfigUpdate,
    cfg_mgr: State<'_, ConfigManager>,
) -> Result<(), String> {
    let arc = cfg_mgr.arc();
    let mut cfg = arc.write().await;
    config.apply_to(&mut cfg);
    crate::config::save_to_file(&cfg)
}

#[tauri::command]
pub async fn load_config(cfg_mgr: State<'_, ConfigManager>) -> Result<AppConfig, String> {
    let arc = cfg_mgr.arc();
    let cfg = arc.read().await;
    Ok(cfg.clone())
}

#[derive(serde::Serialize)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    pub description: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub default: serde_json::Value,
}

#[tauri::command]
pub async fn get_config_path() -> Result<String, String> {
    Ok(crate::config::config_path()
        .to_string_lossy()
        .to_string())
}

#[tauri::command]
pub async fn get_config_schema() -> Result<Vec<ConfigField>, String> {
    Ok(vec![
        ConfigField {
            key: "default_cmd".into(),
            label: "默认命令".into(),
            description: "终端启动时执行的默认命令".into(),
            field_type: "string".into(),
            default: serde_json::json!("claude"),
        },
        ConfigField {
            key: "default_args".into(),
            label: "默认参数".into(),
            description: "默认命令的启动参数（数组 / 空格分隔）".into(),
            field_type: "array".into(),
            default: serde_json::json!(["--permission-mode", "bypassPermissions"]),
        },
        ConfigField {
            key: "default_cwd".into(),
            label: "工作目录".into(),
            description: "终端启动时的默认工作目录".into(),
            field_type: "string".into(),
            default: serde_json::json!("~/.onecode/workspace"),
        },
        ConfigField {
            key: "max_terminals".into(),
            label: "最大终端数".into(),
            description: "允许同时运行的最大终端数量".into(),
            field_type: "number".into(),
            default: serde_json::json!(30),
        },
        ConfigField {
            key: "ring_buffer_max_mb".into(),
            label: "缓冲区大小".into(),
            description: "每个终端环形缓冲区的最大大小（MB）".into(),
            field_type: "number".into(),
            default: serde_json::json!(10),
        },
        ConfigField {
            key: "api_key".into(),
            label: "API Key".into(),
            description: "AI API 的认证密钥".into(),
            field_type: "string".into(),
            default: serde_json::json!(""),
        },
        ConfigField {
            key: "base_url".into(),
            label: "Base URL".into(),
            description: "AI API 的基础 URL（如自建代理）".into(),
            field_type: "string".into(),
            default: serde_json::json!("https://api.anthropic.com"),
        },
        ConfigField {
            key: "model".into(),
            label: "Model".into(),
            description: "AI 模型标识符".into(),
            field_type: "string".into(),
            default: serde_json::json!("claude-sonnet-4-6"),
        },
        ConfigField {
            key: "default_backend".into(),
            label: "默认后端".into(),
            description: "终端使用的 AI 后端内核（claude-code / opencode / codex 等）".into(),
            field_type: "string".into(),
            default: serde_json::json!("claude-code"),
        },
    ])
}

// ── 项目管理 ──────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub dir: String,
    pub description: String,
    #[serde(default)]
    pub backend: Option<String>,
}

/// 保存项目元数据到 ~/.onecode/projects/<id>.json。
/// 同名项目已存在则更新（保留原 id 和 created_at），否则新建。
#[tauri::command]
pub async fn save_project(project: ProjectInfo) -> Result<String, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let projects_dir = std::path::PathBuf::from(&home).join(".onecode/projects");
    std::fs::create_dir_all(&projects_dir).map_err(|e| format!("create projects dir: {e}"))?;

    let now = chrono::Utc::now().to_rfc3339();

    // Check if a project with the same name already exists
    let mut existing_id: Option<String> = None;
    let mut existing_path: Option<std::path::PathBuf> = None;
    let mut existing_created: Option<String> = None;
    if projects_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(true, |e| e != "json") {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                        if val.get("name").and_then(|v| v.as_str()) == Some(&project.name) {
                            existing_id = val
                                .get("id")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            existing_created = val
                                .get("created_at")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            existing_path = Some(path);
                            break;
                        }
                    }
                }
            }
        }
    }

    let (id, created_at) = if let (Some(eid), Some(ec)) = (existing_id, existing_created) {
        // Update existing project — remove old file if filename doesn't match id
        if let Some(old) = &existing_path {
            let expected = projects_dir.join(format!("{}.json", eid));
            if *old != expected {
                let _ = std::fs::remove_file(old);
            }
        }
        (eid, ec)
    } else {
        // New project
        (uuid::Uuid::new_v4().to_string(), now.clone())
    };

    let data = serde_json::json!({
        "id": id,
        "name": project.name,
        "dir": project.dir,
        "description": project.description,
        "backend": project.backend,
        "created_at": created_at,
        "updated_at": now,
    });
    let path = projects_dir.join(format!("{}.json", id));
    let content = serde_json::to_string_pretty(&data).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("write {}: {e}", path.display()))?;

    // Migrate: remove old <name>.json if it exists (from v0 format)
    let old_path = projects_dir.join(format!("{}.json", project.name));
    if old_path.exists() && old_path != path {
        let _ = std::fs::remove_file(&old_path);
        log::info!("[project] migrated {} -> {}", old_path.display(), id);
    }

    log::info!("[project] saved {} ({})", project.name, id);
    Ok(id)
}

/// 列出所有已保存的项目。惰性迁移：旧格式 <name>.json（无 id 字段）自动分配 UUID 并重命名。
#[tauri::command]
pub async fn list_projects() -> Result<Vec<serde_json::Value>, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let projects_dir = std::path::PathBuf::from(&home).join(".onecode/projects");
    if !projects_dir.exists() {
        return Ok(vec![]);
    }
    let mut projects = vec![];
    let entries = std::fs::read_dir(&projects_dir).map_err(|e| format!("read dir: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(true, |e| e != "json") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&content) {
                // Lazy migration: assign UUID if missing
                if val.get("id").is_none() {
                    let id = uuid::Uuid::new_v4().to_string();
                    if let Some(o) = val.as_object_mut() {
                        o.insert("id".to_string(), serde_json::Value::String(id.clone()));
                    }
                    // Write back and rename to <id>.json
                    if let Ok(new_content) = serde_json::to_string_pretty(&val) {
                        let new_path = projects_dir.join(format!("{}.json", id));
                        if std::fs::write(&new_path, &new_content).is_ok() {
                            let _ = std::fs::remove_file(&path);
                            log::info!("[project] migrated {} -> {}", path.display(), id);
                        }
                    }
                }
                projects.push(val);
            }
        }
    }
    // Deduplicate by name AND by dir — keep the latest, remove older duplicates.
    // Same-name or same-dir projects are considered duplicates (prevents
    // two project cards pointing to the same directory from showing).
    {
        let mut seen_name: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut seen_dir: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (i, p) in projects.iter().enumerate() {
            let my_time = p.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(name) = p.get("name").and_then(|v| v.as_str()) {
                if let Some(&prev_i) = seen_name.get(name) {
                    let prev_time = projects[prev_i]
                        .get("updated_at")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if my_time >= prev_time {
                        seen_name.insert(name.to_string(), i);
                    }
                } else {
                    seen_name.insert(name.to_string(), i);
                }
            }
            if let Some(dir) = p.get("dir").and_then(|v| v.as_str()) {
                if !dir.is_empty() {
                    if let Some(&prev_i) = seen_dir.get(dir) {
                        let prev_time = projects[prev_i]
                            .get("updated_at")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if my_time >= prev_time {
                            seen_dir.insert(dir.to_string(), i);
                        }
                    } else {
                        seen_dir.insert(dir.to_string(), i);
                    }
                }
            }
        }
        let keep: std::collections::HashSet<usize> = seen_name
            .values()
            .copied()
            .chain(seen_dir.values().copied())
            .collect();
        // Remove orphaned JSON files for duplicate projects
        let remove_indices: Vec<usize> =
            (0..projects.len()).filter(|i| !keep.contains(i)).collect();
        for &i in &remove_indices {
            if let Some(val) = projects[i].get("id").and_then(|v| v.as_str()) {
                let orphan = projects_dir.join(format!("{}.json", val));
                let _ = std::fs::remove_file(&orphan);
                log::info!(
                    "[project] removed duplicate: {} ({})",
                    projects[i]
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?"),
                    val
                );
            }
        }
        if !remove_indices.is_empty() {
            let mut i = 0usize;
            projects.retain(|_| {
                let keep = remove_indices.binary_search(&i).is_err();
                i += 1;
                keep
            });
        }
    }
    // Sort by created_at descending
    projects.sort_by(|a, b| {
        let ta = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let tb = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        tb.cmp(ta)
    });
    Ok(projects)
}

/// 删除项目元数据（不删除目录和关联终端）。按 name 字段查找并删除对应 JSON。
#[tauri::command]
pub async fn delete_project(name: String) -> Result<(), String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let projects_dir = std::path::PathBuf::from(&home).join(".onecode/projects");
    if !projects_dir.exists() {
        return Err(format!("项目 '{}' 不存在", name));
    }
    let entries = std::fs::read_dir(&projects_dir).map_err(|e| format!("read dir: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(true, |e| e != "json") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if val.get("name").and_then(|v| v.as_str()) == Some(&name) {
                    std::fs::remove_file(&path).map_err(|e| format!("删除失败: {e}"))?;
                    log::info!("[project] deleted {} ({})", name, path.display());
                    return Ok(());
                }
            }
        }
    }
    Err(format!("项目 '{}' 不存在", name))
}

// ── CC Sessions ──────────────────────────────────────────────────

#[tauri::command]
pub async fn cc_sessions_list(
    project_dir: Option<String>,
    cache: State<'_, CcSessionsCache>,
) -> Result<Vec<crate::cc_sessions::CcSession>, String> {
    // No filter by default — show conversations from all projects.
    // Frontend can pass project_dir to filter by a specific project.
    Ok(cache.load(project_dir.as_deref()))
}

// ── 系统信息 ──────────────────────────────────────────────────────────

/// 返回当前用户的 home 目录（跨平台）。
/// HOME (Unix/macOS) / USERPROFILE (Windows) / 回退 "."。
#[tauri::command]
pub async fn get_home_dir() -> Result<String, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    Ok(home)
}

/// 前端调试日志：JS → Rust → 终端 stdout
#[tauri::command]
pub fn debug_log(tag: String, msg: String) {
    log::info!("[js:{tag}] {msg}");
}

// ── 外链 / 编辑器打开（optimization-005 / 007）───────────────────────

/// Ctrl/Cmd+Click 链接：用系统默认浏览器打开外链。
/// 走 std::process 直接调系统 opener（mac `open` / win `start` / linux `xdg-open`），
/// 不依赖 GUI PATH、无需额外插件权限（tauri-plugin-shell 的 open 已被官方标记 deprecated）。
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    open_with_default(&url)
}

/// "用 VS Code 打开"：用本地 VS Code 打开目录/文件。
/// 优先 `code` CLI，失败时按平台回退（mac `open -a`、win `cmd /C code`、linux `xdg-open`）。
#[tauri::command]
pub fn open_in_vscode(path: String) -> Result<(), String> {
    spawn_editor(&path)
}

/// 用系统默认处理器打开 URL/路径（mac `open` / win `start` / linux `xdg-open`）。
fn open_with_default(target: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("open")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open failed: {e}"));
    }
    #[cfg(target_os = "windows")]
    {
        return std::process::Command::new("cmd")
            .args(["/C", "start", "", target])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open failed: {e}"));
    }
    #[cfg(target_os = "linux")]
    {
        return std::process::Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open failed: {e}"));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = target;
        Err("unsupported platform".into())
    }
}

fn spawn_editor(path: &str) -> Result<(), String> {
    // 优先 code CLI（若已装 "Install 'code' in PATH"）。
    // GUI app 的 PATH 受限（mac 上通常不含 /usr/local/bin），此 spawn 多数会失败 → 走平台回退。
    if std::process::Command::new("code")
        .arg(path)
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        // open -a 按应用名定位，不依赖 PATH；VS Code 未安装时 spawn 失败返回错误
        return std::process::Command::new("open")
            .args(["-a", "Visual Studio Code", path])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open VS Code failed: {e}"));
    }
    #[cfg(target_os = "windows")]
    {
        return std::process::Command::new("cmd")
            .args(["/C", "code", path])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open VS Code failed: {e}"));
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("open failed: {e}"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = path;
        Err("unsupported platform".into())
    }
}

// ── Provider 管理（M1：多供应商 + 手动切换）────────────────────────

/// 列出全部供应商 + 目录元数据（前端 F3 / 芯片渲染用）
#[tauri::command]
pub async fn providers_list(
    store: State<'_, ProviderStore>,
) -> Result<crate::providers::ProviderCatalogView, String> {
    let cat = store.arc().read().await.clone();
    // 视图里多带一份「每个供应商算出来的窗口/压缩阈值」——界面要显示它，
    // 用户才知道这个数字是哪来的（未识别型号时必须看得见）。
    Ok(crate::providers::ProviderCatalogView::from_catalog(cat))
}

/// 列出两档预置供应商（新增时下拉带出默认 base_url + 型号）
#[tauri::command]
pub async fn providers_presets() -> Result<Vec<crate::providers::ProviderPreset>, String> {
    Ok(crate::providers::PRESETS.to_vec())
}

/// 新增供应商（预设或自定义四要素）
#[tauri::command]
pub async fn providers_add(
    provider: ProviderInput,
    app: AppHandle,
    cfg_mgr: State<'_, ConfigManager>,
    store: State<'_, ProviderStore>,
) -> Result<crate::providers::Provider, String> {
    let saved = providers::add_provider(&store, provider).await?;
    // P2-3: 首个供应商自动激活后，把 creds 同步到 desktop.json（复用切换链路）
    let was_auto_active = {
        let cat_arc = store.arc();
        let cat = cat_arc.read().await;
        cat.active_provider_id.as_deref() == Some(saved.id.as_str())
    };
    if was_auto_active {
        providers::perform_switch(&app, &cfg_mgr, &store, &saved.id, "manual").await?;
    }
    let _ = app.emit("providers-changed", ());
    Ok(saved)
}

/// 编辑供应商
#[tauri::command]
pub async fn providers_update(
    id: String,
    updates: ProviderUpdate,
    app: AppHandle,
    cfg_mgr: State<'_, ConfigManager>,
    store: State<'_, ProviderStore>,
) -> Result<(), String> {
    providers::update_provider(&store, &cfg_mgr, id, updates).await?;
    let _ = app.emit("providers-changed", ());
    Ok(())
}

/// 删除供应商（删除约束由后端强制：仅剩 1 家 / 当前使用中不可删）
#[tauri::command]
pub async fn providers_delete(
    id: String,
    app: AppHandle,
    store: State<'_, ProviderStore>,
) -> Result<(), String> {
    providers::delete_provider(&store, id).await?;
    let _ = app.emit("providers-changed", ());
    Ok(())
}

/// 测试供应商连通性（AC-F3.3：最小 completion → 延迟 + 状态）
#[tauri::command]
pub async fn providers_test(
    id: String,
    store: State<'_, ProviderStore>,
) -> Result<crate::providers::TestConnectionResult, String> {
    let cat_arc = store.arc();
    let provider = {
        let cat = cat_arc.read().await;
        cat.providers
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| format!("供应商不存在: {id}"))?
    };
    // 先 drop 读锁再 await（curl -m 15 阻塞期间不让写操作等锁）
    Ok(providers::test_connection(&provider).await)
}

/// 手动切换（F2：写 desktop.json + providers.json → 刷新托管 AppConfig → emit provider-switched）
#[tauri::command]
pub async fn providers_switch(
    provider_id: String,
    app: AppHandle,
    cfg_mgr: State<'_, ConfigManager>,
    store: State<'_, ProviderStore>,
) -> Result<(), String> {
    providers::perform_switch(&app, &cfg_mgr, &store, &provider_id, "manual").await
}

/// 切档后刷新指定 slot 的 env（ADR 坑①：pty.restart 复用旧 env，必须先刷新）
/// 前端在 provider-switched 后对每个运行中 tab 先调本命令再 pty_restart。
#[tauri::command]
pub async fn pty_refresh_env(
    id: String,
    state: State<'_, MultiPtyManager>,
    cfg_mgr: State<'_, ConfigManager>,
) -> Result<(), String> {
    let cfg_arc = cfg_mgr.arc();
    let config = cfg_arc.read().await;
    state
        .refresh_env(parse_id(&id)?, &config)
        .await
        .map_err(|e| e.to_string())
}

// ── 产出与证据化验收（目标 ④）────────────────────────────────────────
//
// 产出 = 真文件 + 证据 + 验收状态。前两项由 `deliverables` 模块**自己核对**
// （stat 文件、算 git 改动、给证据分级），第三项**只有人能改** —— 这三个命令
// 都不暴露给模型（模型无法自己把产出标记为"已验收"）。

#[tauri::command]
pub async fn deliverables_scan(cwd: String) -> Result<crate::deliverables::DeliverableScan, String> {
    Ok(crate::deliverables::scan(std::path::Path::new(&cwd)))
}

/// 人显式改验收状态（接受 / 驳回 / 退回待审）。
#[tauri::command]
pub async fn deliverable_set_status(
    cwd: String,
    id: String,
    status: String,
    note: Option<String>,
) -> Result<String, String> {
    crate::deliverables::set_status(std::path::Path::new(&cwd), &id, &status, note.as_deref())
        .map(|p| p.to_string_lossy().into_owned())
}

/// 产出审查门：把事实、证据、问题摊开给人看，人点「接受 / 驳回」。
///
/// 返回值：`Some("accepted")` / `Some("rejected")` / `None`（人取消）。
/// 不传 `id` 时取第一份**待处理**的产出（已接受/已驳回的跳过）。
#[tauri::command]
pub async fn deliverable_review(
    cwd: String,
    id: Option<String>,
    app: AppHandle,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::{
        DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
    };

    let path = std::path::PathBuf::from(&cwd);
    let scan = crate::deliverables::scan(&path);
    if scan.deliverables.is_empty() {
        let msg = if scan.unreadable > 0 {
            format!("这个工作区没有可用的产出记录（另有 {} 份读不出来）。", scan.unreadable)
        } else {
            format!(
                "这个工作区还没有产出记录。\n\n员工的产出协议要求把交付写成\n{}/{}/<id>.json",
                cwd,
                crate::deliverables::DIR
            )
        };
        let app2 = app.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            app2.dialog()
                .message(msg)
                .title("产出审查")
                .kind(MessageDialogKind::Info)
                .blocking_show();
        })
        .await;
        return Ok(None);
    }

    let Some(target) = crate::deliverables::pick_target(&scan, id.as_deref()) else {
        return Ok(None);
    };

    let text = crate::deliverables::review_text(&scan, target);
    let target_id = target.id.clone();
    let app2 = app.clone();
    let answer = tauri::async_runtime::spawn_blocking(move || {
        app2.dialog()
            .message(text)
            .title("产出审查")
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "接受".to_string(),
                "驳回".to_string(),
            ))
            .blocking_show_with_result()
    })
    .await
    .map_err(|e| format!("对话框失败：{e}"))?;

    let decision = match answer {
        MessageDialogResult::Custom(s) if s == "接受" => "accepted",
        MessageDialogResult::Custom(_) => "rejected",
        // 窗口关闭 / 取消 → 不动状态（不猜人的意思）
        _ => return Ok(None),
    };
    crate::deliverables::set_status(&path, &target_id, decision, None)?;
    log::info!("[deliverable] {target_id} -> {decision}");
    Ok(Some(decision.to_string()))
}

/// 单条证据命令的超时。给足编译/测试的余量，又不至于让人干等。
const EVIDENCE_CMD_TIMEOUT_SECS: u64 = 120;

/// 证据验证门：把 agent 声明的可执行证据**跑一遍**，结果写回产出记录。
///
/// ⚠️ 安全边界（必须说清楚，也要**先给人看**）：
/// 这些命令是 agent 写进产出记录的，将以**你的 OS 用户身份、无沙箱**在本机执行。
/// 所以：① 只由人触发（agent 调不到）；② 跑之前把命令原样列出来让人确认；
/// ③ 超时用**进程树**清扫（复用 pty 的 sweep，避免留下孙代）。
#[tauri::command]
pub async fn deliverable_verify(
    cwd: String,
    id: Option<String>,
    app: AppHandle,
) -> Result<Option<crate::deliverables::Verification>, String> {
    use tauri_plugin_dialog::{
        DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
    };

    let path = std::path::PathBuf::from(&cwd);

    // 挑同一份产出（与审查入口同一规则）：否则"人看的是 A、跑的是 B"
    let scan = crate::deliverables::scan(&path);
    let Some(target) = crate::deliverables::pick_target(&scan, id.as_deref()) else {
        let app2 = app.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            app2.dialog()
                .message("这个工作区没有可验证的产出记录。")
                .title("验证证据")
                .kind(MessageDialogKind::Info)
                .blocking_show();
        })
        .await;
        return Ok(None);
    };
    let id = target.id.clone();

    // 先把要跑的命令取出来给**人看**（不先看就跑，等于把执行权交给 agent）
    let commands = crate::deliverables::exec_commands(&path, &id)?;
    if commands.is_empty() {
        let app2 = app.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            app2.dialog()
                .message("这份产出没有声明可执行的证据（kind: exec + command），没有可验证的东西。")
                .title("验证证据")
                .kind(MessageDialogKind::Info)
                .blocking_show();
        })
        .await;
        return Ok(None);
    }

    let mut listing = String::from(
        "将在本机执行下面这些命令（由 agent 写在产出记录里）：\n\n",
    );
    for c in &commands {
        listing.push_str(&format!("    $ {c}\n"));
    }
    listing.push_str(&format!(
        "\n⚠️ 以你的 OS 用户身份执行，**没有沙箱**。\n每条最多 {} 秒，超时会清扫整棵进程树。\n\n确认执行吗？",
        EVIDENCE_CMD_TIMEOUT_SECS
    ));

    let app2 = app.clone();
    let answer = tauri::async_runtime::spawn_blocking(move || {
        app2.dialog()
            .message(listing)
            .title("验证证据")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "执行".to_string(),
                "取消".to_string(),
            ))
            .blocking_show_with_result()
    })
    .await
    .map_err(|e| format!("对话框失败：{e}"))?;

    if !matches!(answer, MessageDialogResult::Custom(ref s) if s == "执行") {
        return Ok(None); // 人取消 → 什么都不做，也不留痕迹
    }

    let path2 = path.clone();
    let id2 = id.clone();
    let verification = tauri::async_runtime::spawn_blocking(move || {
        crate::deliverables::verify_evidence(
            &path2,
            &id2,
            std::time::Duration::from_secs(EVIDENCE_CMD_TIMEOUT_SECS),
        )
    })
    .await
    .map_err(|e| format!("执行失败：{e}"))??;

    // 把结果摆回给人看（不要只说"跑完了"）
    let mut summary = format!(
        "{} 条命令：{} 通过 / {} 未通过\n\n",
        verification.results.len(),
        verification.passed(),
        verification.failed()
    );
    for r in &verification.results {
        let status = if r.timed_out {
            "⏱ 超时".to_string()
        } else {
            match r.exit_code {
                Some(0) => "✅ 0".to_string(),
                Some(c) => format!("❌ {c}"),
                None => "❌ 被杀".to_string(),
            }
        };
        summary.push_str(&format!("{} ({}ms) $ {}\n", status, r.duration_ms, r.command));
        if !r.output_tail.is_empty() {
            for line in r.output_tail.lines().take(10) {
                summary.push_str(&format!("      | {line}\n"));
            }
        }
    }

    let app3 = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        app3.dialog()
            .message(summary)
            .title("验证结果")
            .kind(MessageDialogKind::Info)
            .blocking_show();
    })
    .await;

    log::info!(
        "[deliverable] verify {id} -> {}/{} passed",
        verification.passed(),
        verification.results.len()
    );
    Ok(Some(verification))
}

// ── 任务板（目标 ⑤ 的第一块）────────────────────────────────────────
//
// 治理边界与产出一致：**agent 能写任务定义，但不能裁决状态**。
// `task_set_status` 只有人能调用 → agent 往任务文件里写 `done` 只会被标成"自报"。

#[tauri::command]
pub async fn task_list(cwd: String) -> Result<crate::tasks::TaskBoard, String> {
    Ok(crate::tasks::scan(std::path::Path::new(&cwd)))
}

/// 建一条任务（人触发）。返回新任务 id。
#[tauri::command]
pub async fn task_create(
    cwd: String,
    title: String,
    owner: Option<String>,
    note: Option<String>,
    provider_id: Option<String>,
    model: Option<String>,
) -> Result<String, String> {
    crate::tasks::create_full(
        std::path::Path::new(&cwd),
        &title,
        owner.as_deref().unwrap_or(""),
        note.as_deref().unwrap_or(""),
        &[],
        &[],
        provider_id.as_deref().unwrap_or(""),
        model.as_deref().unwrap_or(""),
    )
}

/// 人的裁决：todo | doing | blocked | review | done | dropped。
#[tauri::command]
pub async fn task_set_status(
    cwd: String,
    id: String,
    status: String,
    note: Option<String>,
) -> Result<String, String> {
    crate::tasks::set_human_status(std::path::Path::new(&cwd), &id, &status, note.as_deref())
        .map(|p| p.to_string_lossy().into_owned())
}

/// 改任务的**定义**（负责人 / 备注 / 关联产出）。状态请走 `task_set_status`。
#[tauri::command]
pub async fn task_assign(
    cwd: String,
    id: String,
    owner: String,
    deliverables: Option<Vec<String>>,
    provider_id: Option<String>,
    model: Option<String>,
) -> Result<String, String> {
    crate::tasks::update_definition_full(
        std::path::Path::new(&cwd),
        &id,
        Some(&owner),
        None,
        deliverables.as_deref(),
        provider_id.as_deref(),
        model.as_deref(),
    )
    .map(|p| p.to_string_lossy().into_owned())
}

/// 把任务板摆出来给人看（只读对话框）。
#[tauri::command]
pub async fn task_board(cwd: String, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    let board = crate::tasks::scan(std::path::Path::new(&cwd));
    let text = crate::tasks::board_text(&board);
    let _ = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .message(text)
            .title("任务板")
            .kind(MessageDialogKind::Info)
            .blocking_show();
    })
    .await;
    Ok(())
}

/// 任务裁决门：挑一条**待验收**（review）或最早的未完成任务，人点「完成 / 退回」。
///
/// `None` = 人取消（不动状态）。
#[tauri::command]
pub async fn task_review(
    cwd: String,
    id: Option<String>,
    app: AppHandle,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::{
        DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
    };

    let path = std::path::PathBuf::from(&cwd);
    let board = crate::tasks::scan(&path);
    let target = match id.as_deref() {
        Some(want) => board.tasks.iter().find(|t| t.id == want),
        None => board
            .tasks
            .iter()
            .find(|t| t.status == "review")
            .or_else(|| board.tasks.iter().find(|t| !matches!(t.status.as_str(), "done" | "dropped"))),
    };

    let Some(target) = target else {
        let _ = tauri::async_runtime::spawn_blocking(move || {
            app.dialog()
                .message("没有待裁决的任务。")
                .title("任务裁决")
                .kind(MessageDialogKind::Info)
                .blocking_show();
        })
        .await;
        return Ok(None);
    };

    let target_id = target.id.clone();
    let mut text = String::new();
    text.push_str(&format!("{}\n\n", target.title));
    text.push_str(&format!("任务 {}，负责人 {}\n", target.id, if target.owner.is_empty() { "(未指派)".to_string() } else { format!("@{}", target.owner) }));
    text.push_str(&format!("当前状态：{}（来源：{}）\n", target.status, if target.status_source == "human" { "人裁决" } else { "任务文件声明" }));
    if !target.note.is_empty() {
        text.push_str(&format!("备注：{}\n", target.note));
    }
    if !target.deliverable_refs.is_empty() {
        text.push_str("\n关联产出：\n");
        for r in &target.deliverable_refs {
            let v = match r.verified {
                Some((p, n)) if p == n => format!("，证据 {p}/{n} ✅"),
                Some((p, n)) => format!("，证据 {p}/{n} ❌"),
                None => "，证据**未验证**".to_string(),
            };
            text.push_str(&format!(
                "  {} [{}]{}  {}\n",
                if r.exists { "·" } else { "❌" },
                r.status,
                v,
                r.id
            ));
        }
        if target.all_deliverables_accepted {
            text.push_str("\n→ 关联产出全部已验收。\n");
        } else {
            text.push_str("\n→ 还有产出没被验收，建议先做产出审查。\n");
        }
    } else {
        text.push_str("\n（这条任务没有关联任何产出 —— 完成前建议先补产出记录。）\n");
    }
    if let Some(p) = &target.problem {
        text.push_str(&format!("\n⚠ {p}\n"));
    }
    text.push_str("\n裁决：「完成」会以你的名义把状态记为 done（不是 agent 自报）。");

    let app2 = app.clone();
    let answer = tauri::async_runtime::spawn_blocking(move || {
        app2.dialog()
            .message(text)
            .title("任务裁决")
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "完成".to_string(),
                "退回重做".to_string(),
            ))
            .blocking_show_with_result()
    })
    .await
    .map_err(|e| format!("对话框失败：{e}"))?;

    let decision = match answer {
        MessageDialogResult::Custom(s) if s == "完成" => "done",
        MessageDialogResult::Custom(_) => "doing",
        _ => return Ok(None),
    };
    crate::tasks::set_human_status(&path, &target_id, decision, None)?;
    log::info!("[task] {target_id} -> {decision} (human)");
    Ok(Some(decision.to_string()))
}

// ── 账本（目标 ⑤ 的第四块）──────────────────────────────────────────
//
// 只报**可测量的事实**：产出侧（任务/产出/验收/证据）来自我们自己的记录，
// 机时侧（token）来自本机 transcript。**金额不算** —— transcript 里没有成本字段，
// 算金额需要价格表；编一个数字出来比不报更糟（账本里会写明这一点）。

/// `full = true` 会做**全库完整扫描**（本机实测约 15 秒），默认 false。
#[tauri::command]
pub async fn ledger_scan(cwd: String, full: Option<bool>) -> Result<crate::ledger::Ledger, String> {
    let p = std::path::Path::new(&cwd);
    Ok(if full.unwrap_or(false) { crate::ledger::scan_full(p) } else { crate::ledger::scan(p) })
}

/// 把账本摆成对话框给人看。`full = true` 时先做全库扫描（约 15 秒）。
#[tauri::command]
pub async fn ledger_show(cwd: String, full: Option<bool>, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    let p = std::path::PathBuf::from(&cwd);
    let want_full = full.unwrap_or(false);
    // 全库扫描是慢路径：放到阻塞线程，别卡住 IPC
    let l = tauri::async_runtime::spawn_blocking(move || {
        if want_full {
            crate::ledger::scan_full(&p)
        } else {
            crate::ledger::scan(&p)
        }
    })
    .await
    .map_err(|e| format!("账本计算失败：{e}"))?;
    let text = crate::ledger::ledger_text(&l);
    let _ = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .message(text)
            .title("账本")
            .kind(MessageDialogKind::Info)
            .blocking_show();
    })
    .await;
    Ok(())
}

// ── 员工模型花名册（目标 ②：按员工自选模型）──────────────────────────
//
// 绑定与实际**分开报**：只给绑定，用户不知道实际跑的是别的；只给实际，
// 用户不知道自己设过什么。两者并列，不一致才看得见。

#[tauri::command]
pub async fn roster_list(
    providers: State<'_, ProviderStore>,
    cfg_mgr: State<'_, ConfigManager>,
) -> Result<Vec<providers::RosterRow>, String> {
    let cat = providers.arc();
    let cat = cat.read().await;
    let cfg = cfg_mgr.arc();
    let cfg = cfg.read().await;
    Ok(providers::roster_view(&cat, &cfg))
}

/// 给某个员工绑定路线。`model` 为空 = 用该 provider 自己的默认模型。
#[tauri::command]
pub async fn roster_set(
    agent_id: String,
    provider_id: String,
    model: Option<String>,
    providers: State<'_, ProviderStore>,
) -> Result<(), String> {
    let agent_id = agent_id.trim().trim_start_matches('@').to_string();
    if agent_id.is_empty() {
        return Err("员工 id 不能为空".to_string());
    }
    let provider_id = provider_id.trim().to_string();
    if provider_id.is_empty() {
        return Err("供应商 id 不能为空".to_string());
    }
    let cat = providers.arc();
    let mut cat = cat.write().await;
    if !cat.providers.iter().any(|p| p.id == provider_id) {
        // 允许绑不存在的供应商吗？不允许 —— 那只会制造一个必然 failover 的配置。
        return Err(format!("供应商 {provider_id} 不在目录里"));
    }
    cat.roster.insert(
        agent_id.clone(),
        providers::RosterEntry {
            provider_id,
            model: model.filter(|m| !m.trim().is_empty()),
        },
    );
    providers::save_to_file(&cat)?;
    log::info!("[roster] {agent_id} 已绑定");
    Ok(())
}

/// 解除某个员工的绑定（回到全局激活供应商）。
#[tauri::command]
pub async fn roster_clear(
    agent_id: String,
    providers: State<'_, ProviderStore>,
) -> Result<bool, String> {
    let agent_id = agent_id.trim().trim_start_matches('@').to_string();
    let cat = providers.arc();
    let mut cat = cat.write().await;
    let removed = cat.roster.remove(&agent_id).is_some();
    if removed {
        providers::save_to_file(&cat)?;
        log::info!("[roster] {agent_id} 已解绑");
    }
    Ok(removed)
}

/// 花名册对话框。
#[tauri::command]
pub async fn roster_show(
    providers: State<'_, ProviderStore>,
    cfg_mgr: State<'_, ConfigManager>,
    app: AppHandle,
) -> Result<(), String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    let cat = providers.arc();
    let cat = cat.read().await;
    let cfg = cfg_mgr.arc();
    let cfg = cfg.read().await;
    let rows = providers::roster_view(&cat, &cfg);
    let text = providers::roster_text(&rows, &cat.providers);
    drop(cfg);
    drop(cat);
    let _ = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .message(text)
            .title("员工模型花名册")
            .kind(MessageDialogKind::Info)
            .blocking_show();
    })
    .await;
    Ok(())
}

// ── 引擎定义（目标 ①：可换引擎）──────────────────────────────────────
//
// 内置 7 个 + `~/.onecode/engines.json` 里的用户定义。加一个新引擎不需要改 Rust
// 重编译 —— 这是"可换引擎的调度台"最低限度的一步。

/// 引擎列表对话框（含来源标记、能力、以及**坏定义的问题清单**）。
#[tauri::command]
pub async fn engines_show(cwd: Option<String>, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    // 项目层（如果这个项目有自己的 `.onecode/engines.json`）也要列出来 ——
    // 它跟着仓库走，不显示就等于"悄悄多了一个引擎"
    let pdir = cwd
        .as_deref()
        .map(|c| crate::backend::project_engines_path(std::path::Path::new(c)));
    let has_project = pdir.as_ref().map(|p| p.exists()).unwrap_or(false);
    let cwd_path = cwd.as_deref().map(std::path::Path::new);
    let reg = crate::backend::Registry::load_layers(&crate::backend::engines_path(), cwd_path);
    let detected = crate::backend::detect_installed_in(cwd_path);
    let text = crate::backend::engines_text(&crate::backend::engines_path(), &detected, &reg.problems);
    let text = if has_project {
        format!(
            "（含项目层：{}）\n\n{text}",
            pdir.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
        )
    } else {
        text
    };
    let _ = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .message(text)
            .title("引擎")
            .kind(MessageDialogKind::Info)
            .blocking_show();
    })
    .await;
    Ok(())
}

/// 引擎配置示例（可直接抄进 `~/.onecode/engines.json`）。
#[tauri::command]
pub async fn engines_example_show(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    let path = crate::backend::engines_path();
    let body = format!(
        "写进这个文件：\n{}\n\n示例内容：\n\n{}\n\n说明：\n· `base` = 从一个内置档继承，只写要覆盖的字段\n· 空串（如 \"system_prompt_file_flag\": \"\"）表示**明确关掉**继承来的能力\n· 不写 base 就从一个空档开始（那就必须写 cmd）\n· 改完不需要重启，下次开终端就会读到",
        path.display(),
        crate::backend::engines_example()
    );
    let _ = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .message(body)
            .title("引擎配置示例")
            .kind(MessageDialogKind::Info)
            .blocking_show();
    })
    .await;
    Ok(())
}

// ── 引擎能力协商（目标 ①）────────────────────────────────────────────
//
// 问**引擎自己**它有什么能力，而不是靠我们在定义里手写。
// 实测依据：`<engine> -p ok --output-format stream-json --verbose` 会在推理之前
// 先吐一行 `system/init`，里面有模型/权限档/凭据来源/工具/agents/skills ——
// 我们读到它立刻杀掉进程，所以**不花 token**。

#[tauri::command]
pub async fn engine_probe(
    engine_id: String,
    cwd: String,
    providers: State<'_, ProviderStore>,
    cfg_mgr: State<'_, ConfigManager>,
) -> Result<crate::engine_probe::ProbeReport, String> {
    let profile = crate::backend::resolve_or_default(&engine_id);
    if profile.cmd.trim().is_empty() {
        return Err(format!("引擎 {} 没有 cmd，无法探测", profile.id));
    }

    // env：与 spawn 同一套（隔离配置目录 + 路由 env）。这里只做探测，
    // 所以不注入员工姿态/技能等 spawn 期参数。
    let mut env: HashMap<String, String> = HashMap::new();
    let cconfig = providers::cc_config_dir();
    std::fs::create_dir_all(&cconfig).ok();
    env.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        cconfig.to_string_lossy().into_owned(),
    );
    {
        let cat = providers.arc();
        let cat = cat.read().await;
        let cfg = cfg_mgr.arc();
        let cfg = cfg.read().await;
        let route = providers::resolve_route_for_agent(&cat, &cfg, None, None, None);
        providers::apply_route_env(&profile.env_key_map, &route, &mut env);
    }

    let path = std::path::PathBuf::from(&cwd);
    let id = profile.id.clone();
    let cmd = profile.cmd.clone();
    let args = profile.default_args.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::engine_probe::probe(
            &id,
            &cmd,
            &args,
            &path,
            &env,
            std::time::Duration::from_secs(20),
        )
    })
    .await
    .map_err(|e| format!("探测失败：{e}"))
}

/// 探测并把结果摆成对话框。
#[tauri::command]
pub async fn engine_probe_show(
    engine_id: String,
    cwd: String,
    providers: State<'_, ProviderStore>,
    cfg_mgr: State<'_, ConfigManager>,
    app: AppHandle,
) -> Result<(), String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    let report = engine_probe(engine_id, cwd, providers, cfg_mgr).await?;
    let text = crate::engine_probe::probe_text(&report);
    let _ = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .message(text)
            .title("引擎能力协商")
            .kind(MessageDialogKind::Info)
            .blocking_show();
    })
    .await;
    Ok(())
}

// ── 结构化跑一次（目标 ① 的非 PTY 路径 + ⑤ 账本的金额）────────────────
//
// 与 PTY 槽位的区别：不走 TUI，而是 `-p <prompt> --output-format stream-json`，
// 于是输出是**结构化事件**、结束时引擎给出 `result.total_cost_usd` 与 `modelUsage`。
// 这条路径让"跑一次花了多少钱"第一次成为可记录的事实（而不是靠价格表估）。

/// 结构化跑一次的默认超时。真实干活可能很久，但不该无限等。
const ENGINE_RUN_TIMEOUT_SECS: u64 = 300;

/// 结构化槽位**回放历史**的字符预算（约 8k token）。
/// 超出就丢最早的几轮，并在提示词里写明丢了几轮。
const STRUCTURED_REPLAY_BUDGET_CHARS: usize = 32 * 1024;

#[tauri::command]
pub async fn engine_run(
    cwd: String,
    prompt: String,
    agent: Option<String>,
    provider_id: Option<String>,
    model: Option<String>,
    // ★ 任务级模型：给了任务 id 就用任务上写的模型（**压过员工花名册**），
    // 并把这次运行记到该任务名下（账本因此能答"这条活花了多少钱"）。
    task_id: Option<String>,
    app: AppHandle,
    cfg_mgr: State<'_, ConfigManager>,
    providers: State<'_, ProviderStore>,
) -> Result<crate::engine_run::RunReport, String> {
    if prompt.trim().is_empty() {
        return Err("提示词不能为空".to_string());
    }

    // ── 任务级覆盖（目标 ②「按任务自选模型」）──
    // 优先级：任务上写死的 > 调用方显式传的 > 员工花名册 > 全局。
    // 任务级之所以要压过花名册：一条任务要换模型，通常正是因为这条活特殊。
    let mut agent = agent;
    let mut provider_id = provider_id;
    let mut model = model;
    let mut label = task_id.clone().unwrap_or_default();
    if let Some(tid) = task_id.as_deref() {
        let board = crate::tasks::scan(std::path::Path::new(&cwd));
        match board.tasks.iter().find(|t| t.id == tid) {
            Some(t) => {
                // 判断只有一处实现（`Task::route_override`）—— 命令层不再自己读字段
                let (p, m) = t.route_override();
                if let Some(p) = p {
                    provider_id = Some(p.to_string());
                }
                if let Some(m) = m {
                    model = Some(m.to_string());
                }
                // 任务有 owner 而调用方没指定员工 → 用任务上的
                if agent.is_none() && !t.owner.is_empty() {
                    agent = Some(t.owner.clone());
                }
                log::info!(
                    "[engine_run] 任务 {tid} 覆盖：provider={:?} model={:?} agent={:?}",
                    provider_id,
                    model,
                    agent
                );
            }
            None => log::warn!("[engine_run] 找不到任务 {tid}，按普通运行处理"),
        }
    }
    if label.is_empty() {
        label = "manual".to_string();
    }

    let cfg_arc = cfg_mgr.arc();
    let config = cfg_arc.read().await;

    // 复用 spawn 计划：身份/姿态/技能/路由/failover 与终端槽位走**同一套**判断
    let req = crate::spawn_plan::SpawnRequest {
        cwd: Some(cwd.clone()),
        agent: agent.clone(),
        provider_id: provider_id.clone(),
        model: model.clone(),
        ..Default::default()
    };
    let plan = {
        let catalog = providers.arc();
        let mut catalog = catalog.write().await;
        let plan = crate::spawn_plan::plan(&req, &config, &catalog);
        if let Some((from, to, reason)) = &plan.switch_record {
            providers::record_switch(&mut catalog, from, to, reason);
        }
        plan
    };
    drop(config);
    for w in &plan.warnings {
        log::warn!("[engine_run] {w}");
    }

    let path = std::path::PathBuf::from(&plan.cwd);
    // 把"到底要跑什么"写进日志：引擎定义可能来自内置 / 全局 / **项目层**，
    // 出问题时第一件事就是确认跑的是哪一个。
    log::info!("[engine_run] {}", crate::spawn_plan::describe(&plan));
    let report = {
        let (cmd, args, env) = (plan.cmd.clone(), plan.args.clone(), plan.env.clone());
        tauri::async_runtime::spawn_blocking(move || {
            crate::engine_run::run(
                &cmd,
                &args,
                &path,
                &env,
                &prompt,
                std::time::Duration::from_secs(ENGINE_RUN_TIMEOUT_SECS),
            )
        })
        .await
        .map_err(|e| format!("运行失败：{e}"))?
    };

    // 记账：**每一次运行都记**，包括"没有结算"的失败。
    // 第 25 刀真机验证时发现：供应商连不上时引擎不产生 result 事件，
    // 于是那次失败**完全没留下痕迹** —— 账本上看起来像"什么都没发生"。
    {
        let (window, model_name, cost, input, output, ms, turns, error) = match &report.result {
            Some(res) => {
                let (w, m) = res
                    .model_usage
                    .first()
                    .map(|m| (m.context_window, m.model.clone()))
                    .unwrap_or((None, plan.model.clone()));
                (
                    w,
                    m,
                    res.total_cost_usd,
                    res.usage.input_tokens,
                    res.usage.output_tokens,
                    res.duration_ms,
                    res.num_turns,
                    if res.is_error {
                        report.error.clone().unwrap_or_default()
                    } else {
                        String::new()
                    },
                )
            }
            None => (
                None,
                plan.model.clone(),
                0.0,
                0,
                0,
                report.wall_ms,
                0,
                report.error.clone().unwrap_or_else(|| "没有结算事件".to_string()),
            ),
        };
        let entry = crate::engine_run::RunEntry {
            at: chrono::Utc::now().to_rfc3339(),
            agent: agent.clone().unwrap_or_default(),
            provider_id: plan.provider_id.clone().unwrap_or_default(),
            model: model_name,
            cost_usd: cost,
            input_tokens: input,
            output_tokens: output,
            duration_ms: ms,
            num_turns: turns,
            ok: report.ok,
            session_id: report
                .result
                .as_ref()
                .map(|r| r.session_id.clone())
                .unwrap_or_default(),
            label,
            context_window: window,
            error,
        };
        if let Err(e) = crate::engine_run::append_run(std::path::Path::new(&cwd), &entry) {
            log::error!("[engine_run] 记账失败（结果仍然返回）：{e}");
        } else {
            log::info!(
                "[engine_run] 记账 ${:.6} ｜ 输入 {} ｜ 输出 {} ｜ ok={}",
                entry.cost_usd,
                entry.input_tokens,
                entry.output_tokens,
                entry.ok
            );
        }
    }

    // ★ 熔断（②）：把这次结果记进连败计数；真的连败到阈值就换档。
    // 判定刻意保守 —— 超时/没有结算一律"说不清"，不动计数（见 providers::classify_run）。
    if let Some(pid) = plan.provider_id.as_deref() {
        let outcome = providers::classify_run(&report);
        if let Some(to) = providers::note_run_outcome(&app, &cfg_mgr, &providers, pid, outcome).await
        {
            log::warn!("[engine_run] 已因连败切换到 {to}");
        }
    }
    Ok(report)
}

/// 结构化跑一次并把结果摆成对话框。
#[tauri::command]
pub async fn engine_run_show(
    cwd: String,
    prompt: String,
    agent: Option<String>,
    task_id: Option<String>,
    app: AppHandle,
    cfg_mgr: State<'_, ConfigManager>,
    providers: State<'_, ProviderStore>,
) -> Result<(), String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    let report = engine_run(cwd, prompt, agent, None, None, task_id, app.clone(), cfg_mgr, providers).await?;
    let text = crate::engine_run::run_text(&report);
    let _ = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .message(text)
            .title("结构化运行结果")
            .kind(MessageDialogKind::Info)
            .blocking_show();
    })
    .await;
    Ok(())
}

// ── 启动期降级（让"只躺在日志里的事"出现在界面上）────────────────────

/// 启动期发生的、影响功能的降级。前端启动时取一次。
#[tauri::command]
pub async fn startup_issues() -> Result<Vec<crate::diagnostics::StartupIssue>, String> {
    Ok(crate::diagnostics::snapshot())
}

/// 自检开关：`ONECODE_SELFCHECK=1` 时前端启动会跑一遍真实命令的自检。
///
/// 为什么由后端给这个开关：这样开关只有一处（环境变量），前端不必猜；
/// 关掉时前端连动态 import 都不做，一行开销都没有。
#[tauri::command]
pub fn selfcheck_enabled() -> bool {
    std::env::var("ONECODE_SELFCHECK").map(|v| v == "1" || v == "true").unwrap_or(false)
}

/// 自检的**开关集合**。
///
/// `network` 单独一个开关，是因为"要花钱的探测"必须**显式**打开：
/// 自检绝不能在用户不知情时替他花掉 API 费用。
#[derive(serde::Serialize)]
pub struct SelfcheckFlags {
    pub enabled: bool,
    /// `ONECODE_SELFCHECK_NETWORK=1` 才允许跑会真的调用模型的探测
    pub network: bool,
}

#[tauri::command]
pub fn selfcheck_flags() -> SelfcheckFlags {
    let on = |k: &str| std::env::var(k).map(|v| v == "1" || v == "true").unwrap_or(false);
    SelfcheckFlags { enabled: on("ONECODE_SELFCHECK"), network: on("ONECODE_SELFCHECK_NETWORK") }
}

// ── 结构化常驻 Slot（目标 ①：可换引擎不该要求引擎长得像终端）──────────
//
// 与 `engine_run`（跑一次就结束）的区别：**固定 session id + 落盘的对话记录**，
// 于是它是一个可以一直聊下去的槽位，而且每轮都有结算。

/// 开一个结构化槽位（生成槽位 id 与请求用的会话 id）。
#[tauri::command]
pub async fn structured_open(
    cwd: String,
    agent: Option<String>,
    provider_id: Option<String>,
    model: Option<String>,
) -> Result<crate::structured_slot::StructuredSlotView, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let session_id = uuid::Uuid::new_v4().to_string();
    crate::structured_slot::open(
        std::path::Path::new(&cwd),
        agent.as_deref().unwrap_or(""),
        provider_id.as_deref().unwrap_or(""),
        model.as_deref().unwrap_or(""),
        &id,
        &session_id,
    )
    .map(Into::into)
}

/// 读一个结构化槽位。
#[tauri::command]
pub async fn structured_state(
    cwd: String,
    id: String,
) -> Result<crate::structured_slot::StructuredSlotView, String> {
    crate::structured_slot::read(std::path::Path::new(&cwd), &id).map(|(s, _)| s.into())
}

/// 列出本项目里的结构化槽位（供前端恢复）。
#[tauri::command]
pub async fn structured_list(
    cwd: String,
) -> Result<Vec<crate::structured_slot::StructuredSlotView>, String> {
    Ok(crate::structured_slot::list(std::path::Path::new(&cwd))
        .0
        .into_iter()
        .map(Into::into)
        .collect())
}

/// 说一句：以该槽位的身份/姿态/技能/路由跑一轮，并把这一轮记进记录。
///
/// 会话 id **固定传下去** —— 多轮接续靠它（实测该 flag 被 CLI 接受）。
#[tauri::command]
pub async fn structured_send(
    cwd: String,
    id: String,
    prompt: String,
    app: AppHandle,
    cfg_mgr: State<'_, ConfigManager>,
    providers: State<'_, ProviderStore>,
) -> Result<crate::structured_slot::StructuredSlotView, String> {
    if prompt.trim().is_empty() {
        return Err("提示词不能为空".to_string());
    }
    let path = std::path::PathBuf::from(&cwd);
    let (slot, _bad) = crate::structured_slot::read(&path, &id)?;

    // 复用 spawn 计划：身份/姿态/技能/路由与终端槽位走**同一套**判断
    let cfg_arc = cfg_mgr.arc();
    let config = cfg_arc.read().await;
    let req = crate::spawn_plan::SpawnRequest {
        cwd: Some(cwd.clone()),
        agent: if slot.agent.is_empty() { None } else { Some(slot.agent.clone()) },
        provider_id: if slot.provider_id.is_empty() { None } else { Some(slot.provider_id.clone()) },
        model: if slot.model.is_empty() { None } else { Some(slot.model.clone()) },
        ..Default::default()
    };
    let plan = {
        let catalog = providers.arc();
        let mut catalog = catalog.write().await;
        let plan = crate::spawn_plan::plan(&req, &config, &catalog);
        if let Some((from, to, reason)) = &plan.switch_record {
            providers::record_switch(&mut catalog, from, to, reason);
        }
        plan
    };
    drop(config);

    // 把会话 id 接在计划参数之后（只有 print 协议用得上；ACP 是长连接会话，
    // 由 `session/new` 给 id，不吃 argv）
    let mut args = plan.args.clone();
    if !plan.is_acp() && !slot.session_id.is_empty() {
        args.push("--session-id".to_string());
        args.push(slot.session_id.clone());
    }
    for w in &plan.warnings {
        log::warn!("[structured] {w}");
    }

    // ★ 多轮上下文**由我们自己拥有**：把之前的对话回放进这一轮的提示词。
    // （实测 `--session-id` 只打标签、不接续 —— 见 structured_slot 模块文档。）
    let prompt_for_run = crate::structured_slot::compose_prompt(
        &slot.turns,
        &prompt,
        STRUCTURED_REPLAY_BUDGET_CHARS,
    );
    // 两条运行路径都要说同一句话（排查"到底跑了什么"的第一手信息）
    log::info!("[structured] {}", crate::spawn_plan::describe(&plan));

    // ★ 按**协议**分流：ACP 引擎不吃 `-p`，硬塞只会得到一个说不清的错误。
    let is_acp = plan.is_acp();
    let (report, acp_report) = if is_acp {
        let (cmd, args2, env, cwd2) = (
            plan.cmd.clone(),
            plan.args.clone(),
            plan.env.clone(),
            plan.cwd.clone(),
        );
        let prompt2 = prompt_for_run.clone();
        let rep = tauri::async_runtime::spawn_blocking(move || {
            crate::acp::run(
                &cmd,
                &args2,
                std::path::Path::new(&cwd2),
                &env,
                &prompt2,
                // 默认拒绝：与第 24 刀的默认姿态审计同一条纪律
                crate::acp::PermissionPolicy::Reject,
                std::time::Duration::from_secs(ENGINE_RUN_TIMEOUT_SECS),
            )
        })
        .await
        .map_err(|e| format!("运行失败：{e}"))?;
        // ACP 没有 print 协议的"结算事件"，用空的 RunReport 占位（账本走 turn 的字段）
        let placeholder = crate::engine_run::RunReport {
            ok: rep.ok,
            cmd: plan.cmd.clone(),
            args: plan.args.clone(),
            cwd: plan.cwd.clone(),
            init: None,
            result: None,
            assistant_turns: 0,
            wall_ms: rep.wall_ms,
            error: rep.error.clone(),
            stderr_tail: rep.stderr_tail.clone(),
            // ACP 的结束方式由它自己的 stopReason 表达；这里只用于占位
            exit_kind: crate::engine_run::ExitKind::Completed,
        };
        (placeholder, Some(rep))
    } else {
        let (cmd, env, cwd_for_run) = (plan.cmd.clone(), plan.env.clone(), plan.cwd.clone());
        let args2 = args.clone();
        let prompt2 = prompt_for_run.clone();
        let rep = tauri::async_runtime::spawn_blocking(move || {
            crate::engine_run::run(
                &cmd,
                &args2,
                std::path::Path::new(&cwd_for_run),
                &env,
                &prompt2,
                std::time::Duration::from_secs(ENGINE_RUN_TIMEOUT_SECS),
            )
        })
        .await
        .map_err(|e| format!("运行失败：{e}"))?;
        (rep, None)
    };

    if let Some(rep) = &acp_report {
        log::info!(
            "[structured] ACP：stop={} 工具调用={} 权限请求={} 花费=${:.6}",
            rep.stop_reason,
            rep.tool_calls.len(),
            rep.permission_requests,
            rep.cost_usd
        );
    }

    let turn = if let Some(rep) = &acp_report {
        crate::structured_slot::turn_from_acp(rep, &prompt)
    } else {
        match &report.result {
        Some(res) => crate::structured_slot::Turn {
            at: chrono::Utc::now().to_rfc3339(),
            prompt: prompt.clone(),
            reply: res.reply.clone(),
            ok: !res.is_error,
            cost_usd: res.total_cost_usd,
            input_tokens: res.usage.input_tokens,
            output_tokens: res.usage.output_tokens,
            duration_ms: res.duration_ms,
            session_id: res.session_id.clone(),
            error: if res.is_error {
                report.error.clone().unwrap_or_default()
            } else {
                String::new()
            },
            extra_meta: format!("print · stop={}", res.subtype),
        },
        // 没有结算也要记一轮 —— **失败的花费同样是花费**，而且"这轮没成"本身是信息
        None => crate::structured_slot::Turn {
            at: chrono::Utc::now().to_rfc3339(),
            prompt: prompt.clone(),
            reply: String::new(),
            ok: false,
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            duration_ms: report.wall_ms,
            session_id: String::new(),
            error: report.error.clone().unwrap_or_else(|| "没有结算".to_string()),
            extra_meta: "print · 没有结算事件".to_string(),
        },
        }
    };
    crate::structured_slot::append_turn(&path, &id, &turn)?;
    // 每一次运行都记进账本（与 engine_run 同一条通路，账本因此也包含槽位开销）。
    // 失败也记：`turn.ok == false` 时金额为 0，但"跑了一次没成"本身是管理信息。
    {
        let (window, model_name, cost, input, output, ms, turns) = match &report.result {
            Some(res) => {
                let (w, m) = res
                    .model_usage
                    .first()
                    .map(|m| (m.context_window, m.model.clone()))
                    .unwrap_or((None, plan.model.clone()));
                (
                    w,
                    m,
                    res.total_cost_usd,
                    res.usage.input_tokens,
                    res.usage.output_tokens,
                    res.duration_ms,
                    res.num_turns,
                )
            }
            None => (
                None,
                plan.model.clone(),
                turn.cost_usd,
                turn.input_tokens,
                turn.output_tokens,
                turn.duration_ms,
                0,
            ),
        };
        let entry = crate::engine_run::RunEntry {
            at: turn.at.clone(),
            agent: slot.agent.clone(),
            provider_id: plan.provider_id.clone().unwrap_or_default(),
            model: model_name,
            cost_usd: cost,
            input_tokens: input,
            output_tokens: output,
            duration_ms: ms,
            num_turns: turns,
            ok: turn.ok,
            session_id: turn.session_id.clone(),
            label: format!("slot:{id}"),
            context_window: window,
            error: turn.error.clone(),
        };
        let _ = crate::engine_run::append_run(&path, &entry);
    }

    // ★ 熔断（②）：与 engine_run 走**同一套**判定与切换
    if let Some(pid) = plan.provider_id.as_deref() {
        let outcome = providers::classify_run(&report);
        if let Some(to) = providers::note_run_outcome(&app, &cfg_mgr, &providers, pid, outcome).await
        {
            log::warn!("[structured] 已因连败切换到 {to}");
        }
    }
    crate::structured_slot::read(&path, &id).map(|(s, _)| s.into())
}

/// 自检专用：给一个**可写的临时项目目录**。
///
/// 为什么需要：自检要真的跑 ④⑤ 的读写命令（任务板/产出/账本/结构化槽位），
/// 但那必须发生在一个**碰不到真实配置**的地方 —— 既不能污染用户的项目，
/// 也不能写到 `~/.onecode/`（供应商/花名册是全局的，自检绝不能改）。
///
/// 只在 `ONECODE_SELFCHECK=1` 时可用：不开放给一般路径。
#[tauri::command]
pub fn selfcheck_scratch_dir() -> Result<String, String> {
    if !selfcheck_enabled() {
        return Err("自检未开启".to_string());
    }
    // 路径是**确定的**（temp_dir/onecode-selfcheck），每次重建：
    // 既避免多次运行堆积，也让自检结果可重复（上次的任务不会污染这次）。
    let dir = std::env::temp_dir().join("onecode-selfcheck");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("清理旧目录失败：{e}"))?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建失败：{e}"))?;
    Ok(dir.to_string_lossy().into_owned())
}
