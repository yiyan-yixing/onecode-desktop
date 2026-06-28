// 自定义滚动条（提取自 gateway/static/term.js 246-360）。
// xterm 原生滚动条隐藏，绘制浮动 thumb + 行号 label。

export class ScrollThumb {
  constructor(term, termEl) {
    this.term = term;
    this.termEl = termEl;
    // 注意：不要设置 position: relative！
    // .term-instance CSS 已设 position:absolute; inset:0，它本身就是 containing block，
    // 覆盖为 relative 会导致元素高度从「填满父容器」变成 auto（内容高度），
    // 这正是输入框逐渐下滑消失的根因——fit() 基于错误的容器高度反复计算。
    // position:absolute + contain:layout paint 已足以让 track 定位正确。

    this.track = document.createElement('div');
    this.track.className = 'xterm-scroll-track';
    this.track.style.cssText =
      'position:absolute;right:0;top:0;bottom:0;width:10px;z-index:10;cursor:pointer';
    termEl.appendChild(this.track);

    this.thumb = document.createElement('div');
    this.thumb.style.cssText =
      'position:absolute;right:2px;top:0;width:3px;min-height:28px;border-radius:4px;' +
      'background:linear-gradient(180deg,rgba(125,211,252,.25),rgba(96,165,250,.35));' +
      'box-shadow:0 0 6px rgba(125,211,252,.1);opacity:0;' +
      'transition:opacity .3s,width .2s,background .2s,right .2s,box-shadow .2s';
    this.track.appendChild(this.thumb);

    this.label = document.createElement('div');
    this.label.style.cssText =
      'position:absolute;right:18px;padding:3px 10px;border-radius:6px;' +
      'background:rgba(10,12,20,.94);color:#c8d6f0;font-size:10px;letter-spacing:.5px;' +
      'font-family:-apple-system,BlinkMacSystemFont,Segoe UI,system-ui,sans-serif;' +
      'pointer-events:none;opacity:0;transition:opacity .25s;z-index:11;white-space:nowrap;' +
      'border:1px solid rgba(125,211,252,.12);box-shadow:0 2px 12px rgba(0,0,0,.4)';
    this.track.appendChild(this.label);

    this.hideTimer = null;
    this.dragging = false;

    // Store bound listeners for cleanup in dispose()
    this._onTrackMouseDown = (e) => this._onTrackClick(e);
    this._onThumbMouseDown = (e) => this._onThumbDrag(e);
    this.track.addEventListener('mousedown', this._onTrackMouseDown);
    this.thumb.addEventListener('mousedown', this._onThumbMouseDown);

    // xterm event listeners (onResize/onScroll return disposable handles)
    this._resizeDisposable = term.onResize(() => setTimeout(() => this.update(), 200));
    this._scrollDisposable = term.onScroll(() => {
      // 节流：避免高频 scroll 事件导致 DOM 回流 → ResizeObserver 循环
      if (!this._scrollRaf) {
        this._scrollRaf = requestAnimationFrame(() => {
          this._scrollRaf = null;
          this.update();
        });
      }
    });
  }

  /** 释放所有事件监听器和 DOM 元素，防止内存泄漏。 */
  dispose() {
    // Clear pending timers
    clearTimeout(this.hideTimer);
    if (this._scrollRaf) {
      cancelAnimationFrame(this._scrollRaf);
      this._scrollRaf = null;
    }
    // Dispose xterm listener handles
    if (this._resizeDisposable) { try { this._resizeDisposable.dispose(); } catch (_) {} }
    if (this._scrollDisposable) { try { this._scrollDisposable.dispose(); } catch (_) {} }
    // Remove DOM event listeners
    if (this._onTrackMouseDown) this.track.removeEventListener('mousedown', this._onTrackMouseDown);
    if (this._onThumbMouseDown) this.thumb.removeEventListener('mousedown', this._onThumbMouseDown);
    // Clean up active drag listeners (in case dispose happens mid-drag)
    if (this._dragOnMove) {
      document.removeEventListener('mousemove', this._dragOnMove);
      document.removeEventListener('mouseup', this._dragOnUp);
      document.body.style.userSelect = '';
      this._dragOnMove = null;
      this._dragOnUp = null;
    }
    // Remove DOM elements
    this.track.remove();
  }

  update() {
    const buf = this.term.buffer.active;
    const total = buf.length;
    const viewY = buf.viewportY;
    const viewH = this.term.rows;
    if (total <= viewH) {
      this.thumb.style.opacity = '0';
      this.label.style.opacity = '0';
      return;
    }
    const ratio = viewH / total;
    const elH = this.termEl.clientHeight;
    const thH = Math.max(24, Math.min(elH * 0.5, elH * ratio));
    const thTop = (viewY / (total - viewH)) * (elH - thH);
    this.thumb.style.height = thH + 'px';
    this.thumb.style.top = thTop + 'px';
    this.thumb.style.opacity = '1';
    this.label.textContent = viewY + 1 + ' / ' + total;
    this.label.style.top = Math.max(0, thTop + thH / 2 - 10) + 'px';
    clearTimeout(this.hideTimer);
    this.hideTimer = setTimeout(() => {
      if (!this.dragging) {
        this.thumb.style.opacity = '0';
        this.label.style.opacity = '0';
      }
    }, 1500);
  }

  _onTrackClick(e) {
    if (e.target === this.thumb) return;
    e.preventDefault();
    const buf = this.term.buffer.active;
    const total = buf.length;
    if (total <= this.term.rows) return;
    const rect = this.track.getBoundingClientRect();
    const target = Math.round(((e.clientY - rect.top) / rect.height) * total);
    try {
      this.term.scrollToLine(target);
    } catch (_) {}
    this.update();
    this.label.style.opacity = '1';
  }

  _onThumbDrag(e) {
    e.preventDefault();
    e.stopPropagation();
    this.dragging = true;
    this.thumb.style.background =
      'linear-gradient(180deg,rgba(125,211,252,.55),rgba(96,165,250,.65))';
    this.thumb.style.boxShadow = '0 0 10px rgba(125,211,252,.25)';
    this.thumb.style.width = '5px';
    this.thumb.style.right = '1px';
    this.label.style.opacity = '1';
    document.body.style.userSelect = 'none';

    const startY = e.clientY;
    const buf = this.term.buffer.active;
    const startViewY = buf.viewportY;
    const total = buf.length;
    const viewH = this.term.rows;
    const trackH = this.termEl.clientHeight;

    // Store as instance fields so dispose() can clean up during active drag
    this._dragOnMove = (ev) => {
      const dy = ev.clientY - startY;
      const linesPerPx = total / trackH;
      let target = Math.round(startViewY + dy * linesPerPx);
      target = Math.max(0, Math.min(total - viewH, target));
      try {
        this.term.scrollToLine(target);
      } catch (_) {}
      this.update();
    };
    this._dragOnUp = () => {
      this.dragging = false;
      this.thumb.style.background = '';
      this.thumb.style.boxShadow = '';
      this.thumb.style.width = '';
      this.thumb.style.right = '';
      document.body.style.userSelect = '';
      document.removeEventListener('mousemove', this._dragOnMove);
      document.removeEventListener('mouseup', this._dragOnUp);
      this._dragOnMove = null;
      this._dragOnUp = null;
    };
    document.addEventListener('mousemove', this._dragOnMove);
    document.addEventListener('mouseup', this._dragOnUp);
  }
}
