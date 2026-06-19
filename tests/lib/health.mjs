// 忠实移植 pty/health.rs 的 report_of 判定逻辑。
// 来源：onecode-desktop/src-tauri/src/pty/health.rs:22-92
// 注：只移植「动作判定」纯逻辑；probe（读 /proc 或 ps）属平台 IO，不在此。

export const RSS_WARN_BYTES = 2 * 1024 * 1024 * 1024; // :22

// 与 Rust HealthAction 一致（serde rename_all lowercase）。
export const ACTIONS = { NONE: 'none', KILL: 'kill', WARN: 'warn', STALE: 'stale' };

// 移植 health.rs:69-81 的 action 计算顺序：zombie → stale → rss。
// @param {object} p  { status: string, alive: bool, isZombie: bool, rssBytes: number|null }
export function healthAction({ status, alive, isZombie, rssBytes }) {
  if (isZombie) return ACTIONS.KILL;
  if (!alive && status === 'running') return ACTIONS.STALE;
  if (rssBytes != null && rssBytes >= RSS_WARN_BYTES) return ACTIONS.WARN;
  return ACTIONS.NONE;
}

// 完整 reportOf（移植 report_of），给定 pid 是否存在 + probe 结果。
export function reportOf(s) {
  if (s.pid == null) {
    return { id: s.id, pid: null, status: s.status, alive: false, isZombie: false, rssBytes: null, action: ACTIONS.NONE };
  }
  // probe 结果由调用方在 s.alive/s.isZombie/s.rssBytes 给定
  const alive = s.alive;
  const rssBytes = s.rssBytes;
  const isZombie = s.isZombie;
  const action = healthAction({ status: s.status, alive, isZombie, rssBytes });
  return { id: s.id, pid: s.pid, status: s.status, alive, isZombie, rssBytes, action };
}
