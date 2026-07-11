//! 系统托盘常驻（P1-1 完整实现）。
//!
//! 行为（见 desktop-prd.md §5.5）：
//! - 关闭窗口 → 隐藏到托盘（不退出应用），终端继续运行。
//! - 托盘左键单击 → 显示并聚焦窗口。
//! - 托盘菜单：新建终端 / 显示窗口 / ── / 退出 OneCode。
//! - 退出：emit `app:before-quit` 让前端保存会话，kill 所有 PTY，退出进程。
//!
//! 注：trayIcon 完全在代码中构建，tauri.conf.json 不声明静态 trayIcon
//!     （否则与 `with_id` 自动创建的同名 tray 冲突）。

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Emitter, Manager, WindowEvent};

use crate::pty::MultiPtyManager;

/// 初始化系统托盘 + 窗口关闭拦截。
pub fn setup(app: &App) -> tauri::Result<()> {
    // ── 菜单项 ──
    let new_term = MenuItem::with_id(app, "tray:new", "新建终端", true, None::<&str>)?;
    let show = MenuItem::with_id(app, "tray:show", "显示窗口", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "tray:quit", "退出 OneCode", true, None::<&str>)?;
    // 逐项 append（避免 with_items 的 &[&T] 单态签名无法统一 MenuItem/PredefinedMenuItem）
    let menu = Menu::new(app)?;
    menu.append(&new_term)?;
    menu.append(&show)?;
    menu.append(&sep)?;
    menu.append(&quit)?;

    // ── 托盘图标（用应用默认图标） ──
    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("OneCode Desktop")
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(on_tray_icon_event);

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;

    // ── 窗口关闭 → 隐藏（不退出） ──
    if let Some(win) = app.get_webview_window("main") {
        let weak = win.clone();
        win.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = weak.hide();
            }
        });
    }

    Ok(())
}

/// 托盘菜单点击。
fn on_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        "tray:new" => {
            show_window(app);
            let _ = app.emit("tray:new-terminal", ()); // 前端 → tabManager.createTab()
        }
        "tray:show" => show_window(app),
        "tray:quit" => {
            // 通知前端关闭所有终端 tab + 保存会话
            let _ = app.emit("app:before-quit", ());
            // 给前端时间完成 closeAllTabs()（每个 tab 需要 ptyKill IPC + dispose）
            // 不等的话前端 closeTab 还没跑完进程就退了，xterm 和 DOM 都不会清理
            std::thread::sleep(std::time::Duration::from_millis(300));
            // 释放阻止休眠的 assertion
            crate::keep_awake::allow_display_sleep();
            crate::keep_awake::allow_idle_sleep();
            // kill 所有 PTY（兜底：前端 closeTab 已 ptyKill，此处处理遗漏的）
            if let Some(mgr) = app.try_state::<MultiPtyManager>() {
                mgr.kill_all_blocking();
            }
            app.exit(0);
        }
        _ => {}
    }
}

/// 托盘图标左键单击 → 显示窗口。
fn on_tray_icon_event(tray: &tauri::tray::TrayIcon, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        show_window(tray.app_handle());
    }
}

fn show_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}
