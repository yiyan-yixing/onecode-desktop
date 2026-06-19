//! 系统托盘（P1/M3 骨架）。
//!
//! 设计来源：desktop-prd.md §5.5、desktop-code-structure.md `tray.rs`。
//! M3 完整实现：关闭窗口 → 隐藏到托盘（不退出）；托盘菜单：新建终端 / 显示窗口 / 退出；
//! 退出时 kill 所有 PTY。
//!
//! 当前：托盘图标已在 `tauri.conf.json` 声明，交互逻辑待 M3 接线。

use tauri::App;

/// 初始化系统托盘。
/// TODO(M3): 用 `tauri::tray::TrayIconBuilder` 创建菜单，
///           绑定 window close-requested → hide（而非退出）。
pub fn setup(_app: &App) -> tauri::Result<()> {
    Ok(())
}
