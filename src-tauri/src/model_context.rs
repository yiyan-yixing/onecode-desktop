//! 模型上下文窗口 —— 供应商必须把「模型真实的窗口」告诉 Claude Code。
//!
//! ## 为什么有这个模块（2026-09-23 董事长实测两起 400）
//!
//! 两起都是**上下文溢出**，根因同一个：供应商条目里没有声明模型窗口，
//! Claude Code 只好按「未识别型号」的默认值办事，于是会话一路涨过 provider 的真实上限：
//!
//! ```text
//! 阿里 qwen3.8-27b ：窗口 262144，会话涨到 392074        → 400
//! deepseek v4 flash：窗口 1048576，请求 1017426 + 补全 32000 = 1049426 > 窗口 → 400
//! ```
//!
//! Claude Code 自己就有这两个旋钮（**从 CLI 二进制里的字符串核实**，不是猜的）：
//!
//! | 旋钮 | 作用 | CLI 原文 |
//! |---|---|---|
//! | `CLAUDE_CODE_MAX_CONTEXT_TOKENS` | 声明**模型真实窗口** | 「set CLAUDE_CODE_MAX_CONTEXT_TOKENS to its real window」 |
//! | `CLAUDE_CODE_AUTO_COMPACT_WINDOW` / settings 键 `autoCompactWindow` | **何时自动压缩** | 「实际阈值 = 该设置与模型最大窗口取小」 |
//!
//! 所以这一层只做一件事：**把窗口变成一个"算出来的数"，而不是一个手填、会漏、会过期的数。**
//! 手填的代价已经付过了——预设里明明写了 `786432`，但董事长实际在用的是**自定义新增**的
//! 同名供应商（`extra_env` 为空），那个值根本没生效。
//!
//! ## 压缩阈值为什么要扣掉补全预算
//!
//! provider 校验的是 **输入 + 补全 ≤ 窗口**（deepseek 那起的原文就是这么算的：
//! `1017426 in the messages, 32000 in the completion`）。只按输入算的话，
//! 「输入没超窗口」和「整单不超窗口」是两件事——照样 400。
//!
//! ## 未识别型号怎么办
//!
//! 给一个**保守**值并**把来源标出来**（`source = "default"`），界面必须显示
//! 「未识别，按 X 处理」。偏高会 400（症状是会话直接报错），偏低只会压缩得早一点
//! （症状是多花点钱）——两害相权，宁可偏低，但**不能装作知道**。

use serde::Serialize;

/// 补全预算。实测：CLI 默认按 32000 申请补全（deepseek 那起的报文原文）。
pub const OUTPUT_RESERVE: u32 = 32_000;

/// 未识别型号的保守默认窗口。宁可压缩得早，也不要撞 provider 的上限。
pub const DEFAULT_WINDOW: u32 = 128_000;

/// 压缩阈值的**暴涨余量**（窗口的百分比）。
///
/// 这一项是**从真实事故里量出来的**，不是拍脑袋：deepseek 那起的阈值是
/// `786432`，而失败请求的输入已经到 `1017426` ——单轮就超出了阈值 **23 万 token**
/// （阈值检查发生在「上一轮结束之后」，这一轮里塞进的大文件/工具输出它根本没看见）。
/// 所以阈值必须给单轮暴涨留出空间，否则「设了阈值」和「不会撞墙」是两件事。
///
/// 25% 这个量级有旁证：DeepSeek 官方给 1M 窗口推荐的阈值正是 768k = 窗口的 75%，
/// 与我们「留 25%」是同一个数。
const SURGE_RESERVE_PERCENT: u32 = 25;

/// 兜底安全余量（窗口的百分比）。
const SAFETY_DIVISOR: u32 = 20; // 5%

const MIN_WINDOW: u32 = 8_192;
const MIN_COMPACT: u32 = 4_096;

/// 有证据的型号窗口表。**只放能指出出处的数**——猜出来的数字比没有更危险。
///
/// 当前两条证据都来自 2026-09-23 的 provider 报错原文（错误信息里直接写了
/// 「maximum context length (N tokens)」），这是最硬的一手来源。
const REGISTRY: &[(&str, u32)] = &[
    ("qwen3.8-27b", 262_144),   // 报错原文：maximum context length (262144 tokens)
    ("deepseek-v4", 1_048_576), // 报错原文：maximum context length is 1048576 tokens
];

/// 窗口解析结果。`source` 是**给用户看的**：他不知道这个数字是哪来的就没法判断对不对。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct WindowPlan {
    /// 模型真实窗口（喂给 `CLAUDE_CODE_MAX_CONTEXT_TOKENS`）
    pub window: u32,
    /// 自动压缩阈值 = 窗口 − 补全预算 − 安全余量（喂给 `autoCompactWindow`）
    pub compact: u32,
    /// override | suffix | registry | default
    pub source: &'static str,
}

/// 从型号名里的后缀读窗口：`deepseek-v4-pro[1m]` / `qwen[256k]`。
///
/// 这是 onecode 自己的既有约定（providers.json 里已经在用 `[1m]`），
/// 也是用户**唯一能一眼看出**的声明方式，所以优先于内置表。
pub fn parse_suffix(model: &str) -> Option<u32> {
    let lower = model.to_ascii_lowercase();
    let start = lower.rfind('[')?;
    let end = lower[start..].find(']')? + start;
    let token = lower[start + 1..end].trim();
    if token.is_empty() {
        return None;
    }
    let (num, mult) = if let Some(n) = token.strip_suffix('k') {
        (n, 1024u64)
    } else if let Some(n) = token.strip_suffix('m') {
        (n, 1024 * 1024)
    } else {
        (token, 1u64) // 纯数字 = 直接是 token 数
    };
    let n: u64 = num.trim().parse().ok()?;
    let total = n.checked_mul(mult)?;
    if total < MIN_WINDOW as u64 || total > 100_000_000 {
        return None; // 明显不是窗口（比如 [2] / [1.5]），别乱认
    }
    Some(total as u32)
}

/// 内置表查窗（大小写不敏感的子串匹配：`Deepseek-v4-flash` 也要能命中）。
pub fn registry_window(model: &str) -> Option<u32> {
    let m = model.to_ascii_lowercase();
    REGISTRY
        .iter()
        .find(|(key, _)| m.contains(key))
        .map(|(_, w)| *w)
}

/// 压缩阈值 = 窗口 − 补全预算 − 单轮暴涨余量 − 兜底余量。
///
/// 三项各有出处（见模块头与上面两个常量的注释）：补全预算来自 provider 报错原文、
/// 暴涨余量来自真实事故量出来的 23 万 token、兜底余量是防抖动。
///
/// 小窗口（本地 8k）会算出很小的值，所以给一个地板——否则算成 0 或负数，
/// 等于每次请求都压缩，模型就没法用了。
pub fn compact_window(window: u32) -> u32 {
    let reserve = OUTPUT_RESERVE.min(window / 4);
    let surge = window * SURGE_RESERVE_PERCENT / 100;
    let safety = window / SAFETY_DIVISOR;
    window
        .saturating_sub(reserve)
        .saturating_sub(surge)
        .saturating_sub(safety)
        .max(MIN_COMPACT)
}

/// 是不是「一方模型」——即 Claude Code **自己认得**的型号（原生 Anthropic / Claude 系）。
///
/// 对这类模型**必须什么都不声明**：CLI 的 `auto` 是「为你的模型调过的」窗口，
/// 我们插一杠子的唯一效果是把窗口**调小**（我们内置表里没有 Claude 型号，
/// 会落到 128k 的保守默认 → 200k 的模型被按 90k 压缩，纯亏）。
///
/// 判据用 base URL + 型号前缀两条：走代理的 Claude 型号 base_url 不是 anthropic.com，
/// 但型号名骗不了人。
pub fn is_first_party(model: &str, base_url: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    let u = base_url.trim().to_ascii_lowercase();
    m.starts_with("claude") || u.contains("anthropic.com")
}

/// 解析一个供应商的窗口：**用户显式值 > 型号后缀 > 内置表 > 保守默认**。
///
/// 优先级这样排的理由：用户显式填的就是他查过的；后缀是他写在型号里的意图；
/// 内置表是我们有出处的；默认值只是兜底，且会被界面标成「未识别」。
pub fn resolve(model: &str, override_window: Option<u32>) -> WindowPlan {
    let explicit = override_window.filter(|w| *w >= MIN_WINDOW);
    let (window, source) = match explicit {
        Some(w) => (w, "override"),
        None => match parse_suffix(model) {
            Some(w) => (w, "suffix"),
            None => match registry_window(model) {
                Some(w) => (w, "registry"),
                None => (DEFAULT_WINDOW, "default"),
            },
        },
    };
    WindowPlan {
        window,
        compact: compact_window(window),
        source,
    }
}

/// 来源的中文说明（界面直接用，别让前端再写一遍映射——两处口径会漂）。
pub fn source_label(source: &str) -> &'static str {
    match source {
        "override" => "你填的",
        "suffix" => "型号后缀",
        "registry" => "内置表",
        _ => "未识别型号（保守值）",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_drives_the_window() {
        // 型号后缀是用户唯一能一眼看出的声明方式，优先于内置表
        assert_eq!(parse_suffix("deepseek-v4-pro[1m]"), Some(1_048_576));
        assert_eq!(parse_suffix("qwen3.8[256k]"), Some(262_144));
        assert_eq!(parse_suffix("m[128K]"), Some(131_072));
        assert_eq!(parse_suffix("m[200000]"), Some(200_000));
        // 不是窗口的方括号不要乱认（别名、参数都长这样）
        assert_eq!(parse_suffix("glm-5.2"), None);
        assert_eq!(parse_suffix("m[2]"), None);
        assert_eq!(parse_suffix("m[abc]"), None);
    }

    #[test]
    fn registry_is_case_insensitive_and_substring() {
        // providers.json 里真实存在的大小写不一致条目（Deepseek-v4-flash）
        assert_eq!(registry_window("Deepseek-v4-flash"), Some(1_048_576));
        assert_eq!(registry_window("qwen3.8-27b"), Some(262_144));
        assert_eq!(registry_window("glm-5.2"), None);
    }

    #[test]
    fn resolve_priority_is_override_suffix_registry_default() {
        // ① 用户填的说了算
        let p = resolve("qwen3.8-27b", Some(32_768));
        assert_eq!((p.window, p.source), (32_768, "override"));
        // ② 后缀优先于内置表（同一个型号，用户明确写了 [64k]）
        let p = resolve("qwen3.8-27b[64k]", None);
        assert_eq!((p.window, p.source), (65_536, "suffix"));
        // ③ 内置表
        let p = resolve("qwen3.8-27b", None);
        assert_eq!((p.window, p.source), (262_144, "registry"));
        // ④ 兜底：**必须标成 default**，界面要显示「未识别」
        let p = resolve("some-new-model", None);
        assert_eq!((p.window, p.source), (DEFAULT_WINDOW, "default"));
        // 明显不合理的覆盖值（0 / 100）当没填，别让它把窗口压死
        assert_eq!(resolve("m", Some(100)).source, "default");
    }

    #[test]
    fn compact_leaves_room_for_completion_and_a_surge() {
        // deepseek 那起：1017426 输入 + 32000 补全 > 1048576 窗口。
        // ① 阈值要扣掉补全预算；② 还要给**单轮暴涨**留量——实测那轮超出阈值 23 万 token。
        let p = resolve("deepseek-v4-flash[1m]", None);
        assert!(p.compact < 1_017_426, "阈值高于本次真实撞墙的输入长度，等于没修");
        // 阈值到窗口之间要能装下观测到的那次暴涨（23 万）
        assert!(
            p.window - p.compact > 230_994,
            "阈值到窗口的余量装不下实测的单轮暴涨"
        );
        assert_eq!(compact_window(1_048_576), 1_048_576 - 32_000 - 262_144 - 52_428);
        assert_eq!(compact_window(262_144), 262_144 - 32_000 - 65_536 - 13_107);
    }

    #[test]
    fn tiny_windows_do_not_go_negative() {
        // 本地小窗口模型：不能算出 0 / 负数（那等于每次请求都压缩）
        assert_eq!(compact_window(4_096), MIN_COMPACT);
        assert!(compact_window(8_192) >= MIN_COMPACT);
        assert!(compact_window(1) >= MIN_COMPACT);
    }

    #[test]
    fn first_party_models_are_left_alone() {
        // Claude Code 自己认得这些型号，它的 auto 比我们内置表准——
        // 我们接手只会把窗口调小（表里没有 Claude → 128k 默认）
        assert!(is_first_party("claude-sonnet-5", "https://api.anthropic.com"));
        assert!(is_first_party("claude-opus-5", "https://my-proxy.test/anthropic"));
        assert!(is_first_party("m", "https://api.anthropic.com"));
        // 第三方不碰：这正是那两起 400 的型号
        assert!(!is_first_party("deepseek-v4-flash[1m]", "https://api.deepseek.com/anthropic"));
        assert!(!is_first_party("qwen3.8-27b", "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic"));
        assert!(!is_first_party("qwen3.8-fast", "http://localhost:11434"));
    }

    #[test]
    fn source_label_covers_every_source() {
        // 界面靠它显示来源；漏一个就会显示空白，用户不知道数字哪来的
        for s in ["override", "suffix", "registry", "default", "什么鬼"] {
            assert!(!source_label(s).is_empty());
        }
    }
}
