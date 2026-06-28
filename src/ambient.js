// 环境透明度控制器 — 8s 无操作后状态栏淡至环境透明度，鼠标/键盘恢复。
// Cowork 布局下侧边栏始终可见，不参与淡出。

export class AmbientController {
  constructor() {
    this.ribbon = null;
    this._timer = null;
    this._idleMs = 8000;
    this._lastReset = 0;
  }

  init() {
    this.ribbon = document.getElementById('ribbon');

    // Combined mousemove handler: reset idle + bottom-edge restore
    document.addEventListener('mousemove', (e) => {
      this._resetIdle();
      if (e.clientY > window.innerHeight - 50) {
        this._setOpacity(1);
      }
    });

    document.addEventListener('keydown', () => this._resetIdle());

    // Start idle timer
    this._resetIdle();
  }

  _resetIdle() {
    // Throttle: skip if reset less than 500ms ago (avoids per-pixel timer churn)
    const now = Date.now();
    if (now - this._lastReset < 500) return;
    this._lastReset = now;

    // Restore full opacity
    this._setOpacity(1);
    if (this._timer) clearTimeout(this._timer);
    this._timer = setTimeout(() => this._enterAmbient(), this._idleMs);
  }

  _enterAmbient() {
    // Check if palette is open — don't fade if interacting
    const overlay = document.getElementById('paletteOverlay');
    if (overlay && overlay.classList.contains('on')) {
      this._timer = setTimeout(() => this._enterAmbient(), 2000);
      return;
    }
    this._setOpacity(0.35);
  }

  _setOpacity(val) {
    if (this.ribbon) {
      this.ribbon.style.transition = 'opacity 1.2s ease-out';
      this.ribbon.style.opacity = val;
    }
  }
}
