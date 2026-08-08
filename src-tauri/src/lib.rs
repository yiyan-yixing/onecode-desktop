//! OneCode Desktop 应用入口：模块声明 + Tauri Builder。

mod macros;

mod backend;
mod cc_sessions;
mod cc_status;
mod commands;
mod config;
mod events;
mod fs_explorer;
mod keep_awake;
mod menu;
mod pty;
mod providers;
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

            // 供应商目录（providers.json，独立单一真相；M1 多供应商管理 + 手动切换）
            let provider_catalog = providers::load_from_file();
            // 启动对账（P1-1）：providers.json 有 active 但 desktop.json 未同步时回写，
            // 确保后续 ConfigManager/watcher 从一致状态启动。
            if let Err(e) = providers::reconcile_active_to_desktop(&provider_catalog) {
                log::warn!("[providers] reconcile failed (P1, non-fatal): {e}");
            }
            let provider_store = providers::ProviderStore::new(provider_catalog);
            app.manage(provider_store);

            // 应用配置（从 ~/.onecode/desktop.json 加载，无则用默认值）
            let app_config = config::load_from_file();
            let cfg_arc = Arc::new(app_config.clone());
            let cfg_mgr = ConfigManager::new(app_config);
            app.manage(cfg_mgr);

            // PTY 管理器（核心）
            let pty_mgr = MultiPtyManager::new(app.handle().clone(), cfg_arc.clone(), runtime);
            app.manage(pty_mgr);
            // P0-1 修复：不再 manage 冻结的 Arc<AppConfig>。pty_spawn / pty_refresh_env
            // 改从 ConfigManager 的 Arc<RwLock<AppConfig>> 读当前值，切档即时生效。

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

            // 原生应用菜单栏（承载快捷键可发现性，P0-4）
            if let Err(e) = menu::setup(app) {
                log::warn!("[menu] setup failed (P0, non-fatal): {e}");
            }

            // 阻止系统空闲休眠（允许屏幕熄灭，合盖仍休眠）
            keep_awake::prevent_idle_sleep();

            // 屏幕防熄屏由 PTY 活跃状态驱动：
            // - spawn() 创建第一个 PTY 时激活（幂等）
            // - kill() 关闭最后一个 PTY 时释放
            // - kill_all_blocking() 退出时释放

            // 健康检测后台循环
            pty::health::start_loop(app.handle().clone());

            // 配置文件热加载（监控 agent 直接修改 ~/.onecode/desktop.json）
            config::start_config_watcher(
                app.state::<ConfigManager>().arc(),
                app.handle().clone(),
            );

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
            commands::pty_set_active,
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
            commands::get_config_path,
            commands::get_config_schema,
            fs_explorer::fs_list_dir,
            fs_explorer::fs_read_file,
            wizard::check_environment,
            wizard::is_first_run,
            wizard::save_wizard_config,
            backend::list_backends,
            commands::debug_log,
            commands::open_external,
            commands::open_in_vscode,
            commands::providers_list,
            commands::providers_presets,
            commands::providers_add,
            commands::providers_update,
            commands::providers_delete,
            commands::providers_test,
            commands::providers_switch,
            commands::pty_refresh_env,
        ])
        .on_menu_event(crate::menu::on_menu_event)
        .run(tauri::generate_context!())
        .expect("failed to run OneCode Desktop");
}
