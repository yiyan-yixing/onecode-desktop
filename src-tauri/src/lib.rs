//! OneCode Desktop 应用入口：模块声明 + Tauri Builder。

mod commands;
mod config;
mod events;
mod pty;
mod session;
mod tray;

use std::sync::Arc;

use tauri::Manager;

use config::AppConfig;
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

            // 会话存储（P1 骨架，初始化失败不阻塞启动）
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

            // 系统托盘（P1 骨架）
            if let Err(e) = tray::setup(app) {
                log::warn!("[tray] setup failed (P1, non-fatal): {e}");
            }

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
        ])
        .run(tauri::generate_context!())
        .expect("failed to run OneCode Desktop");
}
