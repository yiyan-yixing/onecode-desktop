//! 原生应用菜单栏（P0-4）。
//!
//! 状态栏不再平铺快捷键药丸，可发现性 + 加速键迁移至 macOS 菜单栏。
//! 菜单项点击 → emit `menu:<id>` 事件 → 前端 main.js 调用已有 handler
//! （与键盘快捷键共用同一套动作，菜单只是第二条触发路径）。
//!
//! 注：与 tray.rs 的托盘菜单事件分离——托盘走 TrayIconBuilder::on_menu_event，
//! 应用菜单栏走 Builder::on_menu_event（本模块 on_menu_event）。

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{App, AppHandle, Emitter, Manager};

use crate::pty::MultiPtyManager;

/// 生成「切换终端 N」菜单项 ID（纯辅助，DRY）。
fn switch_menu_id(n: u8) -> String {
    format!("menu:switch-{n}")
}

/// 构建并安装原生应用菜单。
pub fn setup(app: &App) -> tauri::Result<()> {
    // ── 应用菜单（macOS 首项，标题会被替换为应用名）──
    let app_menu = Submenu::new(app, "OneCode Desktop", true)?;
    app_menu.append(&PredefinedMenuItem::about(app, Some("关于 OneCode Desktop"), None)?)?;
    app_menu.append(&PredefinedMenuItem::separator(app)?)?;
    app_menu.append(&PredefinedMenuItem::hide(app, Some("隐藏"))?)?;
    app_menu.append(&PredefinedMenuItem::hide_others(app, Some("隐藏其他"))?)?;
    app_menu.append(&PredefinedMenuItem::show_all(app, Some("显示全部"))?)?;
    app_menu.append(&PredefinedMenuItem::separator(app)?)?;
    let app_quit = MenuItem::with_id(app, "menu:quit", "退出 OneCode Desktop", true, Some("CmdOrCtrl+Q"))?;
    app_menu.append(&app_quit)?;

    // ── 终端 ──
    let term_menu = Submenu::new(app, "终端", true)?;
    term_menu.append(&MenuItem::with_id(app, "menu:new", "新建终端", true, Some("CmdOrCtrl+T"))?)?;
    term_menu.append(&MenuItem::with_id(app, "menu:new-in-project", "项目内新建", true, Some("CmdOrCtrl+Shift+T"))?)?;
    term_menu.append(&PredefinedMenuItem::separator(app)?)?;
    term_menu.append(&MenuItem::with_id(app, "menu:close", "关闭终端", true, Some("CmdOrCtrl+W"))?)?;

    // ── 视图 ──
    let view_menu = Submenu::new(app, "视图", true)?;
    view_menu.append(&MenuItem::with_id(app, "menu:toggle-sidebar", "侧栏", true, Some("CmdOrCtrl+B"))?)?;
    view_menu.append(&MenuItem::with_id(app, "menu:toggle-right-panel", "右侧面板", true, Some("CmdOrCtrl+Shift+F"))?)?;

    // ── 窗口：切换终端 1-9 ──
    let win_menu = Submenu::new(app, "窗口", true)?;
    for n in 1..=9u8 {
        win_menu.append(&MenuItem::with_id(
            app,
            &switch_menu_id(n),
            format!("切换到终端 {n}"),
            true,
            Some(&format!("CmdOrCtrl+{n}")),
        )?)?;
    }
    win_menu.append(&PredefinedMenuItem::separator(app)?)?;
    win_menu.append(&PredefinedMenuItem::minimize(app, Some("最小化"))?)?;

    // ── 帮助 ──
    let help_menu = Submenu::new(app, "帮助", true)?;
    help_menu.append(&MenuItem::with_id(app, "menu:help", "快捷键说明…", true, None::<&str>)?)?;

    // 组装主菜单（逐项 append，避免 with_items 的单态签名问题，同 tray.rs）
    let menu = Menu::new(app)?;
    menu.append(&app_menu)?;
    menu.append(&term_menu)?;
    menu.append(&view_menu)?;
    menu.append(&win_menu)?;
    menu.append(&help_menu)?;

    app.set_menu(menu)?;
    Ok(())
}

/// 应用菜单栏事件：转发 `menu:<id>` 给前端；退出走与托盘一致的优雅退出流程。
pub fn on_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();
    if id == "menu:quit" {
        quit_app(app);
        return;
    }
    // 其余菜单项 → 转发前端
    let _ = app.emit(id, ());
}

/// 优雅退出：通知前端保存会话 → 释放防休眠 → kill 所有 PTY → 退出。
/// tray 的「退出」与本模块的 ⌘Q 共用此流程。
pub fn quit_app(app: &AppHandle) {
    let _ = app.emit("app:before-quit", ());
    // 给前端时间完成 closeAllTabs()（每个 tab 需 ptyKill IPC + dispose）
    std::thread::sleep(std::time::Duration::from_millis(300));
    crate::keep_awake::allow_display_sleep();
    crate::keep_awake::allow_idle_sleep();
    if let Some(mgr) = app.try_state::<MultiPtyManager>() {
        mgr.kill_all_blocking();
    }
    app.exit(0);
}
