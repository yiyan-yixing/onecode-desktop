//! OneCode Desktop 应用入口：模块声明 + Tauri Builder。

mod cc_status;
mod commands;
mod config;
mod events;
mod pty;
mod session;
mod tray;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::Manager;

use config::AppConfig;
use cc_status::CcStatusCache;
use pty::MultiPtyManager;
use session::SessionStore;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            // Tauri setup 运行在 tokio runtime 内，可获取 Handle。
            let runtime = tokio::runtime::Handle::try_current()
                .expect("tokio runtime required at setup");

            // 应用配置（M1 用默认值）
            let config = Arc::new(AppConfig::default());
            app.manage(config.clone());

            // PTY 管理器（核心）
            let pty_mgr = MultiPtyManager::new(app.handle().clone(), config, runtime);
            app.manage(pty_mgr);

            // CC Status 缓存（skills/hooks/plugins/tasks/agents，读 ~/.claude）
            let global_dir = std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".claude"))
                .unwrap_or_else(|_| PathBuf::from(".claude"));
            app.manage(CcStatusCache::new(global_dir));

            // 会话存储（P1，初始化失败不阻塞启动——前端启动恢复会捕获缺失并建默认终端）
            match app.path().app_data_dir() {
                Ok(dir) => match SessionStore::new(dir) {
                    Ok(store) => {
                        app.manage(store);
                    }
                    Err(e) => {
                        log::warn!("[session] init failed (P1, non-fatal): {e}");
                    }
                },
                Err(e) => {
                    log::warn!("[session] app_data_dir unavailable (P1, non-fatal): {e}");
                }
            }

            // 系统托盘（P1）
            if let Err(e) = tray::setup(app) {
                log::warn!("[tray] setup failed (P1, non-fatal): {e}");
            }

            // 健康检测后台循环（每 5s 轮询 pid RSS/僵尸）
            pty::health::start_loop(app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::pty_spawn,
            commands::pty_kill,
            commands::pty_restart,
            commands::pty_write,
            commands::pty_resize,
            commands::pty_list,
            commands::pty_rename,
            commands::pty_replay,
            commands::session_save,
            commands::session_restore,
            commands::session_persist,
            commands::cc_status,
            commands::cc_status_invalidate,
            commands::health_check,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run OneCode Desktop");
}
