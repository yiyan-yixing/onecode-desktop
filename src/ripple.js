// 涟漪动效 — Agent 完成/失败时从光球位置发射空间涟漪。

export class RippleController {
  constructor() {
    this.layer = null;
  }

  init() {
    this.layer = document.getElementById('rippleLayer');
  }

  /** 从指定 orb 元素位置发射完成涟漪 */
  emit(orbEl, type = 'success') {
    if (!this.layer || !orbEl) return;

    const rect = orbEl.getBoundingClientRect();
    const ripple = document.createElement('div');
    ripple.className = `ripple ${type === 'failure' ? 'failure' : ''}`;
    ripple.style.left = rect.left + 'px';
    ripple.style.top = rect.top + 'px';
    ripple.style.width = rect.width + 'px';
    ripple.style.height = rect.height + 'px';
    ripple.style.borderRadius = getComputedStyle(orbEl).borderRadius;

    this.layer.appendChild(ripple);
    ripple.addEventListener('animationend', () => ripple.remove());
  }

  /** 通过 orb id 发射 */
  emitById(id, type = 'success') {
    const orb = document.querySelector(`.orb[data-id="${id}"]`);
    this.emit(orb, type);
  }
}
