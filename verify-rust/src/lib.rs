//! 纯逻辑 Rust 编译/测试验证 crate（无 Tauri/GTK 依赖）。
//!
//! 目的：在无系统 GUI 库、无 root 的环境，仍能 `cargo test` 验证生产源码的纯逻辑与
//! 关键第三方 API（portable-pty 0.8）。
//!
//! 方法：
//! - `cc_status` / `slot`：用 `#[path]` 把 `src-tauri/src/` 下**真实生产文件**作为模块纳入，
//!   测试已写在那些文件内的 `#[cfg(test)] mod tests`（子模块可访问其私有 fn）。
//!   → 编译通过即证明这些源文件的类型/语法/依赖 API 调用有效（真编译，非移植）。
//! - `health` / `restart`：源文件依赖 tauri（Emitter/Manager）或混在含 tauri 的模块里，
//!   无法整文件纳入；改为将纯函数逐字拷贝到此并测试（注释标注源行）。
//! - `pty_probe`：复制 mod.rs 的 `create_pty`（portable-pty only，无 tauri），真实 spawn
//!   子进程，验证 portable-pty 0.8 的 openpty/spawn/clone_killer/process_id/
//!   try_clone_reader/wait/ExitStatus 调用——即 README 标注的剩余 API 风险点。
//!
//! 注意：本 crate 不构建 Tauri 应用、不触 GUI；仅为「出包前」把无 GUI 也能验证的部分
//! 真编译并单测一遍，提前暴露语法/类型/API 签名错误。

#![allow(dead_code)]

#[path = "../../src-tauri/src/cc_status.rs"]
mod cc_status;

#[path = "../../src-tauri/src/pty/slot.rs"]
mod slot;

// ── health：源文件含 tauri，无法整文件纳入 → 纯函数逐字拷贝 ──────────
// 源：src-tauri/src/pty/health.rs:22-92。
mod health {
    const RSS_WARN_BYTES: u64 = 2 * 1024 * 1024 * 1024; // health.rs:22

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum HealthAction {
        None,
        Kill,
        Warn,
        Stale,
    }

    struct Probe {
        status: String,
        alive: bool,
        is_zombie: bool,
        rss_bytes: Option<u64>,
    }

    // 逐字移植 health.rs:69-81 的 action 判定顺序：zombie → stale → rss。
    fn action_of(p: &Probe) -> HealthAction {
        if p.is_zombie {
            HealthAction::Kill
        } else if !p.alive && p.status == "running" {
            HealthAction::Stale
        } else if let Some(rss) = p.rss_bytes {
            if rss >= RSS_WARN_BYTES {
                HealthAction::Warn
            } else {
                HealthAction::None
            }
        } else {
            HealthAction::None
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn p(status: &str, alive: bool, is_zombie: bool, rss: Option<u64>) -> Probe {
            Probe { status: status.into(), alive, is_zombie, rss_bytes: rss }
        }

        #[test]
        fn zombie_wins_over_high_rss() {
            assert_eq!(action_of(&p("running", false, true, Some(RSS_WARN_BYTES))), HealthAction::Kill);
        }
        #[test]
        fn not_alive_running_is_stale() {
            assert_eq!(action_of(&p("running", false, false, None)), HealthAction::Stale);
        }
        #[test]
        fn rss_at_threshold_warn() {
            assert_eq!(action_of(&p("running", true, false, Some(RSS_WARN_BYTES))), HealthAction::Warn);
        }
        #[test]
        fn rss_just_below_threshold_none() {
            assert_eq!(action_of(&p("running", true, false, Some(RSS_WARN_BYTES - 1))), HealthAction::None);
        }
        #[test]
        fn exited_and_dead_not_stale() {
            assert_eq!(action_of(&p("exited", false, false, None)), HealthAction::None);
        }
    }
}

// ── restart：源在 pty/mod.rs（含 tauri）→ 退避公式逐字拷贝 ───────────
// 源：src-tauri/src/pty/mod.rs:466-470。
mod restart {
    const MAX_RESTART_COUNT: u32 = 10; // mod.rs:45

    // 逐字移植 mod.rs:466-470：exp=(new_count-1).min(20)；delay=500*(1<<exp)，.min(30000)
    fn delay_ms(new_count: u32) -> u64 {
        let exp = (new_count - 1).min(20) as u32;
        500u64
            .saturating_mul(1u64.checked_shl(exp).unwrap_or(u64::MAX / 2))
            .min(30000)
    }
    fn should_give_up(prior_count: u32) -> bool {
        prior_count >= MAX_RESTART_COUNT
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn series_matches_pty_js() {
            let expected = [500, 1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000, 30000];
            for (i, want) in expected.iter().enumerate() {
                assert_eq!(delay_ms((i + 1) as u32), *want, "count={}", i + 1);
            }
        }
        #[test]
        fn cap_30s_for_large_count() {
            assert_eq!(delay_ms(50), 30000);
        }
        #[test]
        fn give_up_at_10() {
            for c in 0..10 {
                assert!(!should_give_up(c));
            }
            assert!(should_give_up(10));
            assert!(should_give_up(11));
        }
    }
}

// ── pty_probe：复制 mod.rs create_pty（portable-pty only），真 spawn 验证 API ─
// 源：src-tauri/src/pty/mod.rs:305-356。
mod pty_probe {
    use std::collections::HashMap;
    use std::io::Read;

    use anyhow::{anyhow, Result};
    use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

    const DEFAULT_COLS: u16 = 200; // mod.rs:36
    const DEFAULT_ROWS: u16 = 50; // mod.rs:37

    struct PtyHandles {
        master: Box<dyn MasterPty + Send>,
        killer: Box<dyn ChildKiller + Send>,
        reader: Box<dyn Read + Send>,
        child: Box<dyn Child + Send + Sync>,
        pid: Option<u32>,
    }

    /// 逐字移植 src-tauri/src/pty/mod.rs:314-356 create_pty（去掉 anyhow 包装一致）。
    fn create_pty(cmd: &str, args: &[String], cwd: &str, env: &HashMap<String, String>) -> Result<PtyHandles> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows: DEFAULT_ROWS, cols: DEFAULT_COLS, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| anyhow!("openpty failed: {e}"))?;
        let mut cb = CommandBuilder::new(cmd);
        for a in args {
            cb.arg(a);
        }
        if !cwd.is_empty() {
            cb.cwd(cwd);
        }
        cb.env("TERM", "xterm-256color");
        for (k, v) in env {
            cb.env(k, v);
        }
        // portable-pty 0.8: spawn_command 在 slave 上。
        let child = pair.slave.spawn_command(cb).map_err(|e| anyhow!("spawn failed: {e}"))?;
        let killer = child.clone_killer(); // portable-pty 0.8: Child::clone_killer
        let pid = child.process_id(); // portable-pty 0.8: Child::process_id -> Option<u32>
        let master = pair.master;
        let reader = master.try_clone_reader().map_err(|e| anyhow!("clone_reader failed: {e}"))?;
        Ok(PtyHandles { master, killer, reader, child, pid })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::HashMap;
        use std::time::Duration;

        /// 验证 portable-pty 0.8：openpty / spawn / clone_killer / process_id /
        /// try_clone_reader / Child::wait / ExitStatus::success 全链路可用。
        #[test]
        fn spawn_and_wait_exercises_pty_api() {
            let env = HashMap::new();
            let mut h = create_pty("/bin/sh", &["-c".into(), "true".into()], "", &env)
                .expect("create_pty should succeed");
            assert!(h.pid.is_some(), "process_id() should return Some(pid)");
            let status = h.child.wait().expect("child.wait should succeed");
            assert!(status.success(), "child should exit successfully");
            // clone_killer 已在 create_pty 调用通过编译即可证明 ChildKiller API 正确
            let _ = &h.killer;
        }

        /// 验证 reader 真能读到子进程输出（openpty→spawn→try_clone_reader→read 闭环）。
        /// 用线程 + 3s 超时读取，避免 master 持有导致无 EOF 时挂起整个测试。
        #[test]
        fn reader_yields_child_output() {
            let env = HashMap::new();
            let mut h = create_pty("/bin/sh", &["-c".into(), "echo pty_reader_ok".into()], "", &env)
                .expect("create_pty");
            let (tx, rx) = std::sync::mpsc::channel();
            let mut reader = h.reader;
            std::thread::spawn(move || {
                let mut s = String::new();
                let _ = reader.read_to_string(&mut s);
                let _ = tx.send(s);
            });
            let _ = h.child.wait();
            let out = rx
                .recv_timeout(Duration::from_secs(3))
                .expect("reader should produce output within 3s");
            assert!(out.contains("pty_reader_ok"), "reader output missing marker; got: {out:?}");
        }
    }
}
