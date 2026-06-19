// Prevents an additional console window on Windows in release builds.
// OneCode Desktop 仅支持 macOS/Linux，此属性仅为跨平台编译兼容。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    onecode_desktop_lib::run();
}
