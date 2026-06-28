//! CC Status — 读取本地 Claude Code 配置（skills/hooks/plugins/tasks/agents）。
//!
//! 移植自 `onecode/agent-runtime/gateway/cc-status.js` 的 `loadCcStatusAsync`。
//!
//! 读取范围：
//! - 全局：`$HOME/.claude`
//! - 项目：`{cwd}/.claude`（前端传当前活跃终端 cwd）
//!
//! 用途：
//! - 状态栏徽章（skills/hooks/plugins/tasks 计数）→ P1-7。
//! - @mention 的 agent 列表 → P1-6。
//!
//! 实现刻意只用 std + serde_json（不引入 regex/walkdir），frontmatter/cron 手动解析。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{Datelike, Timelike};
use serde_json::Value;

const TTL: Duration = Duration::from_secs(5);

#[derive(serde::Serialize, Clone, Default)]
pub struct CcStatus {
    pub skills: Vec<SkillInfo>,
    pub hooks: HashMap<String, Vec<HookInfo>>,
    pub plugins: Vec<PluginInfo>,
    pub tasks: Vec<TaskInfo>,
    pub agents: Vec<AgentInfo>,
}

#[derive(serde::Serialize, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub scope: String,
}

#[derive(serde::Serialize, Clone)]
pub struct HookInfo {
    #[serde(rename = "type")]
    pub kind: String,
    pub command: String,
    pub message: String,
    pub scope: String,
}

#[derive(serde::Serialize, Clone)]
pub struct PluginInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub command: String,
    pub args: Vec<String>,
    pub scope: String,
}

#[derive(serde::Serialize, Clone)]
pub struct TaskInfo {
    pub name: String,
    pub prompt: String,
    pub cron: String,
    pub recurring: Option<bool>,
    pub next_run: String,
    pub scope: String,
}

#[derive(serde::Serialize, Clone)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub tools: String,
    pub model: String,
    pub color: String,
    pub icon: String,
    pub scope: String,
}

/// TTL 缓存（对齐 JS 的 5s TTL）。
/// key = project_dir（None 表示仅全局）。命中即返回缓存，过期则同步重算（文件小，开销可忽略）。
pub struct CcStatusCache {
    global_dir: PathBuf,
    slot: Mutex<Option<(Instant, HashMap<String, CcStatus>)>>,
}

impl CcStatusCache {
    pub fn new(global_dir: PathBuf) -> Self {
        Self {
            global_dir,
            slot: Mutex::new(None),
        }
    }

    /// 读取（带 TTL）。project_dir 为 None 时仅读全局。
    pub fn load(&self, project_dir: Option<&Path>) -> CcStatus {
        let key = project_dir
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let now = Instant::now();
        {
            let guard = self.slot.lock().expect("cc cache poisoned");
            if let Some((ts, map)) = guard.as_ref() {
                if now.duration_since(*ts) < TTL {
                    if let Some(v) = map.get(&key) {
                        return v.clone();
                    }
                }
            }
        }
        // miss / 过期 → 重算
        let status = load_cc_status(&self.global_dir, project_dir);
        let mut guard = self.slot.lock().expect("cc cache poisoned");
        let entry = guard.get_or_insert_with(|| (now, HashMap::new()));
        entry.0 = now;
        entry.1.insert(key, status.clone());
        status
    }

    /// 强制清空缓存（前端「刷新」用）。
    pub fn invalidate(&self) {
        *self.slot.lock().expect("cc cache poisoned") = None;
    }
}

fn load_cc_status(global_dir: &Path, project_dir: Option<&Path>) -> CcStatus {
    let mut out = CcStatus::default();

    // 两层 scope，顺序与 JS 一致：project 先、global 后
    let scopes: Vec<(&Path, &str)> = match project_dir {
        Some(p) => vec![(global_dir, "global"), (p, "project")],
        None => vec![(global_dir, "global")],
    };
    for (dir, scope) in scopes {
        load_skills(dir, scope, &mut out.skills);
        load_hooks(dir, scope, &mut out.hooks);
        load_plugins(dir, scope, &mut out.plugins);
        load_tasks(dir, scope, &mut out.tasks);
        load_agents(dir, scope, &mut out.agents);
    }
    out
}

// ── 加载器 ──────────────────────────────────────────────────────────

fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn read_json(path: &Path) -> Option<Value> {
    read_text(path).and_then(|s| serde_json::from_str(&s).ok())
}

fn load_skills(dir: &Path, scope: &str, out: &mut Vec<SkillInfo>) {
    scan_skills(&dir.join("skills"), scope, out);
}

/// 递归扫描 skills 目录（对齐 JS scanDir：目录无 SKILL.md 则继续下钻）。
fn scan_skills(dir: &Path, scope: &str, out: &mut Vec<SkillInfo>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let fp = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            let skill_md = fp.join("SKILL.md");
            if let Some(content) = read_text(&skill_md) {
                out.push(skill_from_content(&name, &content, scope));
            } else {
                scan_skills(&fp, scope, out);
            }
        } else if fp.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Some(content) = read_text(&fp) {
                let stem = name.trim_end_matches(".md");
                out.push(skill_from_content(stem, &content, scope));
            }
        }
    }
}

fn skill_from_content(fallback_name: &str, content: &str, scope: &str) -> SkillInfo {
    if let Some(fm) = frontmatter_body(content) {
        // fm_field 返回 String（空串=未找到），非 Option：空则回退到 fallback_name。
        let name = {
            let n = fm_field(&fm, "name");
            if n.is_empty() {
                fallback_name.to_string()
            } else {
                n
            }
        };
        let description = fm_field(&fm, "description");
        SkillInfo {
            name,
            description,
            scope: scope.to_string(),
        }
    } else {
        let first_heading = content
            .lines()
            .find(|l| l.trim_start().starts_with('#'))
            .map(|l| l.trim_start_matches('#').trim().to_string())
            .unwrap_or_default();
        SkillInfo {
            name: fallback_name.to_string(),
            description: first_heading,
            scope: scope.to_string(),
        }
    }
}

fn load_hooks(dir: &Path, scope: &str, out: &mut HashMap<String, Vec<HookInfo>>) {
    for f in ["settings.json", "settings.local.json"] {
        let Some(s) = read_json(&dir.join(f)) else {
            continue;
        };
        let Some(hooks) = s.get("hooks").and_then(|v| v.as_object()) else {
            continue;
        };
        for (event, hook_list) in hooks {
            let Some(arr) = hook_list.as_array() else {
                continue;
            };
            let bucket = out.entry(event.clone()).or_default();
            for h in arr {
                let Some(subs) = h.get("hooks").and_then(|v| v.as_array()) else {
                    continue;
                };
                for sub in subs {
                    bucket.push(HookInfo {
                        kind: sub
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        command: sub
                            .get("command")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        message: sub
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        scope: scope.to_string(),
                    });
                }
            }
        }
    }
}

fn load_plugins(dir: &Path, scope: &str, out: &mut Vec<PluginInfo>) {
    for f in ["settings.json", "settings.local.json"] {
        let Some(s) = read_json(&dir.join(f)) else {
            continue;
        };
        let Some(servers) = s.get("mcpServers").and_then(|v| v.as_object()) else {
            continue;
        };
        for (name, cfg) in servers {
            out.push(PluginInfo {
                name: name.clone(),
                kind: cfg
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                command: cfg
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                args: cfg
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                scope: scope.to_string(),
            });
        }
    }
}

fn load_tasks(dir: &Path, scope: &str, out: &mut Vec<TaskInfo>) {
    for f in ["scheduled_tasks.json", "cron-session.json"] {
        let Some(raw) = read_json(&dir.join(f)) else {
            continue;
        };
        let tasks = match (&raw, raw.get("tasks")) {
            (Value::Array(_), _) => raw.as_array().cloned().unwrap_or_default(),
            (_, Some(t)) if t.is_array() => t.as_array().cloned().unwrap_or_default(),
            _ => continue,
        };
        for t in tasks {
            let name = t
                .get("name")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| {
                    t.get("prompt")
                        .and_then(|v| v.as_str())
                        .map(|s| s.chars().take(60).collect())
                        .unwrap_or_default()
                });
            let cron = t
                .get("cron")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            out.push(TaskInfo {
                name,
                prompt: t
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                next_run: cron_next(&cron),
                cron,
                recurring: t.get("recurring").and_then(|v| v.as_bool()),
                scope: scope.to_string(),
            });
        }
    }
}

fn load_agents(dir: &Path, scope: &str, out: &mut Vec<AgentInfo>) {
    let agents_dir = dir.join("agents");
    let entries = match std::fs::read_dir(&agents_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || !name.ends_with(".md") {
            continue;
        }
        let Some(content) = read_text(&entry.path()) else {
            continue;
        };
        let Some(fm) = frontmatter_body(&content) else {
            continue;
        };
        let agent_name = fm_field(&fm, "name");
        if agent_name.is_empty() {
            continue;
        }
        out.push(AgentInfo {
            id: name.trim_end_matches(".md").to_string(),
            name: agent_name,
            description: fm_field(&fm, "description"),
            tools: fm_field(&fm, "tools"),
            model: fm_field(&fm, "model"),
            color: fm_field(&fm, "color"),
            icon: fm_field(&fm, "icon"),
            scope: scope.to_string(),
        });
    }
}

// ── frontmatter / cron 解析 ─────────────────────────────────────────

/// 提取 `---\n...\n---` 之间的 frontmatter 正文（行级解析，无需 regex）。
fn frontmatter_body(content: &str) -> Option<String> {
    let mut lines = content.lines();
    let first = lines.next()?;
    if first.trim() != "---" {
        return None;
    }
    let mut body = Vec::new();
    for l in lines {
        if l.trim() == "---" {
            return Some(body.join("\n"));
        }
        body.push(l);
    }
    None
}

/// 从 frontmatter 取 `key:` 字段值（trim）。
fn fm_field(fm: &str, key: &str) -> String {
    let prefix = format!("{key}:");
    for line in fm.lines() {
        if let Some(rest) = line.trim_start().strip_prefix(&prefix) {
            return rest.trim().to_string();
        }
    }
    String::new()
}

/// cron 下次运行时间（人类可读），移植自 cc-status.js `cronNext`。
/// 解析失败或无匹配返回空串。
fn cron_next(expr: &str) -> String {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return String::new();
    }
    let mins = match expand(fields[0], 0, 59) {
        Some(v) => v,
        None => return String::new(),
    };
    let hrs = match expand(fields[1], 0, 23) {
        Some(v) => v,
        None => return String::new(),
    };
    let doms = match expand(fields[2], 1, 31) {
        Some(v) => v,
        None => return String::new(),
    };
    let mons = match expand(fields[3], 1, 12) {
        Some(v) => v,
        None => return String::new(),
    };
    let dows = match expand(fields[4], 0, 6) {
        Some(v) => v,
        None => return String::new(),
    };

    let now = chrono::Local::now().naive_local();
    // start = 当前分钟截断到秒 0，再 +1 分钟（等价 JS 的 Y,M,D,H,min+1,0，且会正确进位）
    let start = chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), now.day())
        .and_then(|d| d.and_hms_opt(now.hour(), now.minute(), 0))
        .map(|t| t + chrono::Duration::minutes(1))
        .unwrap_or(now);

    let dom_is_star = fields[2] == "*";
    let dow_is_star = fields[4] == "*";

    let doms_vec: Vec<u32> = doms.iter().copied().collect();
    let mons_vec: Vec<u32> = mons.iter().copied().collect();
    let dom_min = *doms_vec.iter().min().unwrap_or(&1);

    // 提前剪枝：最小月份无法容纳最小日
    let max_day_by_month = [0u32, 31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let possible = mons_vec
        .iter()
        .any(|&m| max_day_by_month[m as usize] >= dom_min);
    if !possible {
        return String::new();
    }

    let mut iters = 0u32;
    let mut cursor = start;
    while (cursor - start).num_days() < 366 {
        let (y, mo, d) = (cursor.year(), cursor.month(), cursor.day());
        let dow = cursor.weekday().num_days_from_sunday();
        if !mons.contains(&mo) {
            cursor = next_day(cursor);
            continue;
        }
        let day_ok = if dom_is_star && dow_is_star {
            doms.contains(&d) || dows.contains(&dow)
        } else if dom_is_star || dow_is_star {
            doms.contains(&d) && dows.contains(&dow)
        } else {
            doms.contains(&d) || dows.contains(&dow)
        };
        if !day_ok {
            cursor = next_day(cursor);
            continue;
        }
        for &hr in &hrs {
            for &mi in &mins {
                let cand = chrono::NaiveDate::from_ymd_opt(y, mo, d)
                    .and_then(|date| date.and_hms_opt(hr, mi, 0));
                let Some(cand) = cand else { continue };
                if cand < start {
                    continue;
                }
                iters += 1;
                if iters > 500 {
                    return String::new();
                }
                let secs = cand.signed_duration_since(now).num_seconds();
                if secs < 60 {
                    return "< 1m".into();
                }
                if secs < 3600 {
                    return format!("{}m", secs / 60);
                }
                if secs < 86400 {
                    return format!("{}h {}m", secs / 3600, (secs % 3600) / 60);
                }
                let days = secs / 86400;
                return format!("{}d {}h", days, (secs % 86400) / 3600);
            }
        }
        cursor = next_day(cursor);
    }
    String::new()
}

fn next_day(dt: chrono::NaiveDateTime) -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(dt.year(), dt.month(), dt.day())
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|midnight| midnight + chrono::Duration::days(1))
        .unwrap_or(dt)
}

/// 展开单个 cron 字段为值集合（对齐 JS expand：支持 * , - /）。
fn expand(field: &str, lo: i32, hi: i32) -> Option<std::collections::BTreeSet<u32>> {
    let mut set = std::collections::BTreeSet::new();
    for part in field.split(',') {
        if part == "*" {
            for i in lo..=hi {
                set.insert(i as u32);
            }
        } else if let Some((range, step_s)) = part.split_once('/') {
            let step: i32 = step_s.parse().ok().filter(|&s: &i32| s >= 1)?;
            let (rlo, rhi) = if range == "*" {
                (lo, hi)
            } else if let Some((a, b)) = range.split_once('-') {
                (a.parse().ok()?, b.parse().ok()?)
            } else {
                let v: i32 = range.parse().ok()?;
                (v, v)
            };
            let mut i = rlo;
            while i <= rhi {
                if i >= lo && i <= hi {
                    set.insert(i as u32);
                }
                i += step;
            }
        } else if let Some((a, b)) = part.split_once('-') {
            let a: i32 = a.parse().ok()?;
            let b: i32 = b.parse().ok()?;
            for i in a.max(lo)..=b.min(hi) {
                set.insert(i as u32);
            }
        } else {
            let v: i32 = part.parse().ok()?;
            if v >= lo && v <= hi {
                set.insert(v as u32);
            } else {
                return None;
            }
        }
    }
    Some(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eq_set(got: std::collections::BTreeSet<u32>, expected: &[u32]) {
        let exp: std::collections::BTreeSet<u32> = expected.iter().copied().collect();
        assert_eq!(got, exp);
    }

    #[test]
    fn expand_star_full_range() {
        let s = expand("*", 0, 59).expect("*");
        assert_eq!(s.len(), 60);
    }
    #[test]
    fn expand_single_range_comma_step() {
        eq_set(expand("5", 0, 59).unwrap(), &[5]);
        eq_set(expand("1-5", 0, 59).unwrap(), &[1, 2, 3, 4, 5]);
        eq_set(expand("1,3,5", 0, 59).unwrap(), &[1, 3, 5]);
        eq_set(expand("*/15", 0, 59).unwrap(), &[0, 15, 30, 45]);
        eq_set(expand("10-30/5", 0, 59).unwrap(), &[10, 15, 20, 25, 30]);
    }
    #[test]
    fn expand_out_of_range_or_invalid_is_none() {
        assert!(expand("60", 0, 59).is_none()); // 单值越界
        assert!(expand("5,99", 0, 59).is_none()); // 一项非法整体失败
        assert!(expand("abc", 0, 59).is_none());
        assert!(expand("*/0", 0, 59).is_none()); // step 0 拒绝（与 JS 原版不同）
    }

    #[test]
    fn cron_next_field_count_and_invalid() {
        assert_eq!(cron_next("* * *"), "");
        assert_eq!(cron_next(""), "");
        assert_eq!(cron_next("* * * * * *"), "");
        assert_eq!(cron_next("60 0 * * *"), "");
        assert_eq!(cron_next("0 24 * * *"), "");
        assert_eq!(cron_next("* * * * 7"), "");
    }
    #[test]
    fn cron_next_impossible_day_pruned() {
        assert_eq!(cron_next("0 0 31 2 *"), "");
        assert_eq!(cron_next("0 0 30 2 *"), "");
        assert_eq!(cron_next("0 0 31 4 *"), "");
    }
    #[test]
    fn cron_next_possible_day_nonempty() {
        assert!(!cron_next("0 0 31 * *").is_empty());
        assert!(!cron_next("0 0 30 * *").is_empty());
    }
    #[test]
    fn cron_next_every_minute_under_1m() {
        assert_eq!(cron_next("* * * * *"), "< 1m");
    }

    #[test]
    fn frontmatter_body_basic() {
        assert_eq!(
            frontmatter_body("---\nname: foo\ndescription: bar\n---\n# body"),
            Some("name: foo\ndescription: bar".to_string())
        );
        assert_eq!(frontmatter_body("# no fm"), None);
        assert_eq!(frontmatter_body("---\nname: x"), None); // 无闭合
    }
    #[test]
    fn fm_field_lookup() {
        let fm = "name: Arch\nmodel: opus";
        assert_eq!(fm_field(fm, "name"), "Arch");
        assert_eq!(fm_field(fm, "model"), "opus");
        assert_eq!(fm_field(fm, "color"), "");
        assert_eq!(fm_field("names: x\nname: y", "name"), "y"); // 前缀精确
    }
}
