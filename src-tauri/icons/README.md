# Icons

应用图标未随仓库提交（二进制文件）。

## 生成图标

准备一张 ≥ 1024×1024 的 PNG（建议正方形、透明背景），然后执行：

```bash
# 在仓库根目录
npx @tauri-apps/cli icon ./path/to/app-icon.png --output ./src-tauri/icons
```

`tauri icon` 会一次性生成 Tauri 所需的全部尺寸：

- `32x32.png` / `128x128.png` / `128x128@2x.png` — Linux/Windows
- `icon.icns` — macOS
- `icon.png` — 通用 + 系统托盘
- `icon.ico` — Windows（本仓库不支持 Windows，可忽略）

> 缺失图标会导致 `tauri build` 失败、`tauri dev` 启动时托盘无法显示。
> 在补齐图标前，可临时注释 `tauri.conf.json` 中的 `app.trayIcon` 与 `bundle.icon`。
