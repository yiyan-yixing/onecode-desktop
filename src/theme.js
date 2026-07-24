// 主题管理器 — 暖亮工作区外壳（linen/cream）+ 局部深色终端井（abyss）。
//
// 范式：暖色承载 chrome（标题栏 / 左右侧栏 / 状态栏 / 面板），
// 深色专供终端区域，二者以渐变软过渡衔接（见 styles.css .term-panel）。
// data-theme="dark" 仅作用于终端深色面，非全局暗色——保留属性供未来扩展。

export class ThemeManager {
  init() {
    document.documentElement.setAttribute('data-theme', 'dark');
    // 预留：未来如支持亮/暗切换，在此监听系统偏好。
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener('change', () => {
      document.documentElement.setAttribute('data-theme', 'dark');
    });
  }
}
