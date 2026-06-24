//! CC Sessions — 读取本地 Claude Code 会话数据。
//!
//! 数据源：
//! - `~/.claude/sessions/{pid}.json` — 活跃进程元数据（sessionId, cwd, status, name）
//! - `~/.claude/projects/{projectDir}/{sessionId}.jsonl` — 对话转录
//! - `~/.claude/.session-stats.json` — 工具调用统计
//!
//! 项目目录名编码：`/Users/zhanglei/foo` → `-Users-zhanglei-foo`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

const TTL: Duration = Duration::from_secs(5);

#[derive(serde::Serialize, Clone)]
pub struct CcSession {
    pub session_id: String,
    pub title: String,
    pub project_dir: String,
    pub project_name: String,
    pub status: String,        // "idle" | "busy" | "ended"
    pub first_message: String,
    pub tool_count: u32,
    pub started_at: i64,       // epoch ms
    pub updated_at: i64,       // epoch ms
    pub is_active: bool,
    pub pid: Option<u32>,
}

pub struct CcSessionsCache {
    global_dir: PathBuf,
    slot: Mutex<Option<(Instant, Vec<CcSession>)>>,
}

impl CcSessionsCache {
    pub fn new(global_dir: PathBuf) -> Self {
        Self {
            global_dir,
            slot: Mutex::new(None),
        }
    }

    pub fn load(&self, project_dir: Option<&str>) -> Vec<CcSession> {
        let now = Instant::now();
        {
            let guard = self.slot.lock().expect("cc sessions cache poisoned");
            if let Some((ts, sessions)) = guard.as_ref() {
                if now.duration_since(*ts) < TTL {
                    // If project_dir filter is requested, filter cached results
                    if let Some(pd) = project_dir {
                        return sessions
                            .iter()
                            .filter(|s| s.project_dir == pd)
                            .cloned()
                            .collect();
                    }
                    return sessions.clone();
                }
            }
        }
        // miss / expired → recompute
        let sessions = compute_sessions(&self.global_dir);
        {
            let mut guard = self.slot.lock().expect("cc sessions cache poisoned");
            *guard = Some((now, sessions.clone()));
        }
        if let Some(pd) = project_dir {
            return sessions.into_iter().filter(|s| s.project_dir == pd).collect();
        }
        sessions
    }

    #[allow(dead_code)]
    pub fn invalidate(&self) {
        let mut guard = self.slot.lock().expect("cc sessions cache poisoned");
        *guard = None;
    }
}

// ── Core Logic ──────────────────────────────────────────────────

fn compute_sessions(global_dir: &Path) -> Vec<CcSession> {
    // 1. Read active sessions from ~/.claude/sessions/{pid}.json
    let active_map = load_active_sessions(global_dir);

    // 2. Read session stats from ~/.claude/.session-stats.json
    let stats_map = load_session_stats(global_dir);

    // 3. Scan project directories for transcript files
    let projects_dir = global_dir.join("projects");
    let mut sessions = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&projects_dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let project_path = entry.path();
            scan_project_transcripts(&project_path, &active_map, &stats_map, &mut sessions);
        }
    }

    // 4. Add any active sessions not found in project transcripts
    // (e.g., very new sessions that haven't written transcript yet)
    let seen_ids: std::collections::HashSet<String> =
        sessions.iter().map(|s| s.session_id.clone()).collect();
    for (sid, info) in &active_map {
        if !seen_ids.contains(sid) {
            let cwd_basename = info
                .cwd
                .split('/')
                .last()
                .unwrap_or(&info.cwd)
                .to_string();

            // Try to read title from transcript as fallback (session may have just started)
            let title = info.name.clone().unwrap_or_else(|| {
                let encoded_cwd = info.cwd.replace('/', "-");
                let project_dir = global_dir.join("projects").join(format!("-{}", encoded_cwd));
                let transcript_path = project_dir.join(format!("{}.jsonl", sid));
                if let Ok(content) = std::fs::read_to_string(&transcript_path) {
                    let mut found_title = String::new();
                    let mut found_msg = String::new();
                    for line in content.lines().take(30) {
                        if let Ok(v) = serde_json::from_str::<Value>(line) {
                            let msg_type = v["type"].as_str().unwrap_or("");
                            if msg_type == "ai-title" {
                                if let Some(t) = v["aiTitle"].as_str() {
                                    found_title = t.to_string();
                                }
                            }
                            if msg_type == "user"
                                && found_msg.is_empty()
                                && !v["isMeta"].as_bool().unwrap_or(false)
                                && v["origin"]["kind"].as_str() == Some("human")
                            {
                                if let Some(text) = v["message"]["content"].as_str() {
                                    found_msg = truncate(text, 40);
                                } else if let Some(arr) = v["message"]["content"].as_array() {
                                    for item in arr {
                                        if item["type"].as_str() == Some("text") {
                                            if let Some(t) = item["text"].as_str() {
                                                found_msg = truncate(t, 40);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !found_title.is_empty() {
                        found_title
                    } else if !found_msg.is_empty() {
                        found_msg
                    } else {
                        format!("{} · 新会话", cwd_basename)
                    }
                } else {
                    format!("{} · 新会话", cwd_basename)
                }
            });

            sessions.push(CcSession {
                session_id: sid.clone(),
                title,
                project_dir: info.cwd.clone(),
                project_name: cwd_basename,
                status: info.status.clone(),
                first_message: String::new(),
                tool_count: 0,
                started_at: info.started_at,
                updated_at: info.updated_at,
                is_active: true,
                pid: Some(info.pid),
            });
        }
    }

    // 5. Sort by updated_at descending
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions
}

struct ActiveSessionInfo {
    pid: u32,
    #[allow(dead_code)]
    session_id: String,
    cwd: String,
    status: String, // "idle" | "busy"
    name: Option<String>,
    started_at: i64,
    updated_at: i64,
}

fn load_active_sessions(global_dir: &Path) -> HashMap<String, ActiveSessionInfo> {
    let sessions_dir = global_dir.join("sessions");
    let mut map = HashMap::new();

    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // Filename is {pid}.json
            let pid: u32 = match path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse().ok())
            {
                Some(p) => p,
                None => continue,
            };

            if let Some(v) = read_json(&path) {
                let session_id = v["sessionId"].as_str().unwrap_or("").to_string();
                if session_id.is_empty() {
                    continue;
                }
                let cwd = v["cwd"].as_str().unwrap_or("").to_string();
                let status = v["status"].as_str().unwrap_or("idle").to_string();
                let name = v["name"].as_str().map(|s| s.to_string());
                let started_at = v["startedAt"].as_i64().unwrap_or(0);
                let updated_at = v["updatedAt"].as_i64().unwrap_or(started_at);

                map.insert(
                    session_id.clone(),
                    ActiveSessionInfo {
                        pid,
                        session_id,
                        cwd,
                        status,
                        name,
                        started_at,
                        updated_at,
                    },
                );
            }
        }
    }

    map
}

fn load_session_stats(global_dir: &Path) -> HashMap<String, SessionStats> {
    let stats_path = global_dir.join(".session-stats.json");
    let mut map = HashMap::new();

    if let Some(v) = read_json(&stats_path) {
        if let Some(sessions) = v["sessions"].as_object() {
            for (sid, data) in sessions {
                let tool_count = data["total_calls"].as_u64().unwrap_or(0) as u32;
                let started_at = data["started_at"].as_i64().unwrap_or(0) * 1000; // seconds → ms
                let updated_at = data["updated_at"].as_i64().unwrap_or(0) * 1000;

                map.insert(
                    sid.clone(),
                    SessionStats {
                        tool_count,
                        started_at,
                        updated_at,
                    },
                );
            }
        }
    }

    map
}

struct SessionStats {
    tool_count: u32,
    started_at: i64,
    updated_at: i64,
}

fn scan_project_transcripts(
    project_path: &Path,
    active_map: &HashMap<String, ActiveSessionInfo>,
    stats_map: &HashMap<String, SessionStats>,
    out: &mut Vec<CcSession>,
) {
    // Decode project dir name back to path: -Users-zhanglei-foo → /Users/zhanglei/foo
    let project_dir_name = project_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let decoded_cwd = decode_project_dir(project_dir_name);
    let cwd_basename = decoded_cwd
        .split('/')
        .last()
        .unwrap_or("")
        .to_string();

    if let Ok(entries) = std::fs::read_dir(project_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            // Extract sessionId from filename: {sessionId}.jsonl
            let session_id = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };

            // Skip subagent files
            if session_id.contains("agent-") {
                continue;
            }

            // Parse transcript header (first ~30 lines)
            let (title, first_message, last_timestamp) = parse_transcript_header(&path);

            // Get stats
            let stats = stats_map.get(&session_id);
            let tool_count = stats.map(|s| s.tool_count).unwrap_or(0);
            let started_at = stats
                .map(|s| s.started_at)
                .unwrap_or_else(|| last_timestamp);
            let updated_at = stats
                .map(|s| s.updated_at)
                .unwrap_or(last_timestamp);

            // Check if active
            let active_info = active_map.get(&session_id);
            let is_active = active_info.is_some();
            let (status, pid) = if let Some(info) = active_info {
                (info.status.clone(), Some(info.pid))
            } else {
                ("ended".to_string(), None)
            };

            out.push(CcSession {
                session_id: session_id.clone(),
                title,
                project_dir: decoded_cwd.clone(),
                project_name: cwd_basename.clone(),
                status,
                first_message,
                tool_count,
                started_at,
                updated_at,
                is_active,
                pid,
            });
        }
    }
}

/// Parse the first ~30 lines and last ~5 lines of a transcript JSONL
/// Returns (title, first_user_message, last_timestamp_ms)
fn parse_transcript_header(path: &Path) -> (String, String, i64) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (String::new(), String::new(), 0),
    };

    let mut title = String::new();
    let mut first_message = String::new();
    let mut last_timestamp: i64 = 0;
    let mut line_count = 0;

    for line in content.lines() {
        line_count += 1;
        if line_count > 30 && !first_message.is_empty() && !title.is_empty() {
            break; // We have enough from the header
        }

        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Always track last timestamp
        if let Some(ts) = v["timestamp"].as_str() {
            last_timestamp = parse_iso_timestamp(ts);
        }

        let msg_type = v["type"].as_str().unwrap_or("");

        // Extract title from ai-title
        if msg_type == "ai-title" {
            if let Some(t) = v["aiTitle"].as_str() {
                title = t.to_string();
            }
        }

        // Extract agent-name as fallback title
        if msg_type == "agent-name" && title.is_empty() {
            if let Some(n) = v["agentName"].as_str() {
                title = n.to_string();
            }
        }

        // Extract first user message
        if msg_type == "user" && first_message.is_empty() {
            // Skip meta messages
            if v["isMeta"].as_bool().unwrap_or(false) {
                continue;
            }
            // Check origin is human
            let origin_kind = v["origin"]["kind"].as_str().unwrap_or("");
            if origin_kind != "human" {
                continue;
            }
            // Extract text content
            if let Some(content) = v["message"]["content"].as_str() {
                first_message = truncate(content, 80);
            } else if let Some(arr) = v["message"]["content"].as_array() {
                for item in arr {
                    if item["type"].as_str() == Some("text") {
                        if let Some(text) = item["text"].as_str() {
                            first_message = truncate(text, 80);
                            break;
                        }
                    }
                }
            }
        }
    }

    // Title fallback chain: first_message → session slug
    if title.is_empty() {
        if !first_message.is_empty() {
            // Use the first user message as a meaningful title
            let max_len = 40;
            if first_message.len() > max_len {
                let mut end = max_len;
                while end > 0 && !first_message.is_char_boundary(end) {
                    end -= 1;
                }
                title = format!("{}…", &first_message[..end]);
            } else {
                title = first_message.clone();
            }
        } else {
            title = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Session")
                .split('-')
                .next()
                .unwrap_or("Session")
                .to_string();
        }
    }

    (title, first_message, last_timestamp)
}

/// Decode project directory name back to absolute path.
/// `-Users-zhanglei-foo` → `/Users/zhanglei/foo`
fn decode_project_dir(name: &str) -> String {
    if name.starts_with('-') {
        let path: String = name[1..].replace('-', "/");
        format!("/{}", path)
    } else {
        name.to_string()
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find a safe UTF-8 boundary
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

fn parse_iso_timestamp(ts: &str) -> i64 {
    // Parse ISO 8601 like "2026-06-22T14:13:52.682Z"
    // Use simple parsing since we don't need sub-second precision
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0)
}

// ── Helpers ──────────────────────────────────────────────────

fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn read_json(path: &Path) -> Option<Value> {
    read_text(path).and_then(|s| serde_json::from_str(&s).ok())
}
