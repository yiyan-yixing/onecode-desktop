//! OneCode Desktop 应用入口：模块声明 + Tauri Builder。

mod cc_sessions;
mod cc_status;
mod commands;
mod config;
mod events;
mod fs_explorer;
mod pty;
mod session;
mod tray;
mod wizard;

use std::path::PathBuf;
use std::sync::Arc;

use tauri::Manager;

use cc_sessions::CcSessionsCache;
use cc_status::CcStatusCache;
use config::ConfigManager;
use pty::MultiPtyManager;
use session::SessionStore;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化日志
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let runtime = tauri::async_runtime::handle().inner().clone();

            // 开发调试：打开 WebView 检查器（仅 debug 构建）
            #[cfg(debug_assertions)]
            if let Some(w) = app.get_webview_window("main") {
                w.open_devtools();
            }

            // 应用配置（从 ~/.onecode/desktop.json 加载，无则用默认值）
            let app_config = config::load_from_file();
            let cfg_arc = Arc::new(app_config.clone());
            let cfg_mgr = ConfigManager::new(app_config);
            app.manage(cfg_mgr);

            // PTY 管理器（核心）
            let pty_mgr = MultiPtyManager::new(app.handle().clone(), cfg_arc.clone(), runtime);
            app.manage(pty_mgr);

            // pty_spawn 用的只读配置
            app.manage(cfg_arc);

            // CC Status 缓存
            let global_dir = std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".claude"))
                .unwrap_or_else(|_| PathBuf::from(".claude"));
            app.manage(CcStatusCache::new(global_dir.clone()));
            app.manage(CcSessionsCache::new(global_dir));

            // 会话存储
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

            // 系统托盘
            if let Err(e) = tray::setup(app) {
                log::warn!("[tray] setup failed (P1, non-fatal): {e}");
            }

            // 健康检测后台循环
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
            commands::cc_sessions_list,
            commands::health_check,
            commands::save_config,
            commands::load_config,
            commands::save_project,
            commands::list_projects,
            commands::delete_project,
            commands::get_home_dir,
            fs_explorer::fs_list_dir,
            fs_explorer::fs_read_file,
            wizard::check_environment,
            wizard::is_first_run,
            wizard::save_wizard_config,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run OneCode Desktop");
}
