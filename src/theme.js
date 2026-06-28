// 主题管理器 — Aurora 范式仅支持暗色模式。
// 暗色是玻璃/深度设计的基础，极光只在暗面上才有意义。

export class ThemeManager {
  init() {
    document.documentElement.setAttribute('data-theme', 'dark');
    // System dark mode listener (already dark, but for consistency)
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener('change', () => {
      document.documentElement.setAttribute('data-theme', 'dark');
    });
  }
}
