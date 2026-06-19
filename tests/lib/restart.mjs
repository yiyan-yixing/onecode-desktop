// 忠实移植 pty/mod.rs 的自动重启退避公式。
// 来源：onecode-desktop/src-tauri/src/pty/mod.rs:458-476

const MAX_RESTART_COUNT = 10; // :45
const STABLE_RESET_SECS = 5;

// 是否放弃重启（移植 mod.rs:458-463）。
// Rust: count = 当前 restart_count（inc 之前）；if count >= 10 → 放弃。
// @param priorCount  自增前的计数（即上一次崩溃后的累计重启数）
export function shouldGiveUp(priorCount) {
  return priorCount >= MAX_RESTART_COUNT;
}

// 退避毫秒（移植 mod.rs:466-470）。
// Rust: new_count = inc_restart()（自增后，1..N）；
//   exp = (new_count-1).min(20);
//   delay = 500 * (1u64 << exp)  (checked_shl)，.min(30000)
// @param newCount  自增后的计数（首次重启=1）
export function delayMs(newCount) {
  const exp = Math.min(newCount - 1, 20);
  // JS 1<<exp 是 32 位有符号；exp<=20 → 1048576，正数，与 Rust u64 此范围一致
  const shifted = 1 << exp;
  const raw = 500 * shifted;
  return Math.min(raw, 30000);
}

// 稳定判定（移植 mod.rs:447-456）：spawn 后运行时长 >= 5s 视为稳定 → 重置计数。
export function isStable(elapsedMs) {
  return elapsedMs >= STABLE_RESET_SECS * 1000;
}

// 生成前 N 次重启的退避序列（用于批量断言）。
export function backoffSeries(max = 12) {
  const out = [];
  for (let n = 1; n <= max; n++) out.push(delayMs(n));
  return out;
}
