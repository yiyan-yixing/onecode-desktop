// 忠实移植 cc_status.rs 的 cron_next / expand / next_day（用于逻辑验证）。
// 来源：onecode-desktop/src-tauri/src/cc_status.rs:349-497
// 注：Rust 用 chrono；此处用 Date。语义保持一致（Local.now → 当地时间）。
// 本文件用于在无 cargo 环境验证 Rust 算法逻辑正确性。

const WEEKDAY_FROM_SUNDAY = (d) => d.getDay(); // JS getDay(): 0=周日..6=周六，等价 chrono num_days_from_sunday

// 展开单个 cron 字段（移植 cc_status.rs:457 expand）。
// 返回 Set<number>；解析失败返回 null（对应 Rust 的 None）。
export function expand(field, lo, hi) {
  const set = new Set();
  for (const part of String(field).split(',')) {
    if (part === '*') {
      for (let i = lo; i <= hi; i++) set.add(i);
    } else if (part.includes('/')) {
      const idx = part.indexOf('/');
      const range = part.slice(0, idx);
      const stepS = part.slice(idx + 1);
      // Rust: step_s.parse().ok().filter(|&s| s>=1) —— 非法/0 返回 None（整体失败）
      const stepN = Number(stepS);
      if (!Number.isInteger(stepN) || stepN < 1) return null;
      const step = stepN;
      let rlo, rhi;
      if (range === '*') {
        rlo = lo; rhi = hi;
      } else if (range.includes('-')) {
        const [a, b] = range.split('-');
        rlo = parseIntSafe(a); rhi = parseIntSafe(b);
        if (rlo === null || rhi === null) return null;
      } else {
        const v = parseIntSafe(range);
        if (v === null) return null;
        rlo = v; rhi = v;
      }
      let i = rlo;
      while (i <= rhi) {
        if (i >= lo && i <= hi) set.add(i);
        i += step;
      }
    } else if (part.includes('-')) {
      const [a, b] = part.split('-');
      const av = parseIntSafe(a); const bv = parseIntSafe(b);
      if (av === null || bv === null) return null;
      for (let i = Math.max(av, lo); i <= Math.min(bv, hi); i++) set.add(i);
    } else {
      const v = parseIntSafe(part);
      if (v === null) return null;
      if (v >= lo && v <= hi) set.add(v);
      else return null; // Rust：单值越界 → None
    }
  }
  return set;
}

// Rust 的 part.parse::<i32>().ok()：仅整数，不容忍空白/小数。
function parseIntSafe(s) {
  const t = s.trim();
  if (!/^-?\d+$/.test(t)) return null;
  return parseInt(t, 10);
}

// 移植 cc_status.rs:449 next_day：回到当天 00:00 再 +1 天。
function nextDay(d) {
  const m = new Date(d.getFullYear(), d.getMonth(), d.getDate(), 0, 0, 0, 0);
  return new Date(m.getTime() + 86400000);
}

// 移植 cc_status.rs:349 cron_next。
// 返回人类可读字符串；解析失败/无匹配返回 ''。
export function cronNext(expr) {
  const fields = String(expr || '').trim().split(/\s+/);
  if (fields.length !== 5) return '';
  const mins = expand(fields[0], 0, 59);
  if (!mins) return '';
  const hrs = expand(fields[1], 0, 23);
  if (!hrs) return '';
  const doms = expand(fields[2], 1, 31);
  if (!doms) return '';
  const mons = expand(fields[3], 1, 12);
  if (!mons) return '';
  const dows = expand(fields[4], 0, 6);
  if (!dows) return '';

  const now = new Date();
  // start = 当地当前分:00 + 1 分钟（Rust: and_hms(h,min,0) + Duration::minutes(1)）
  const start = new Date(
    now.getFullYear(), now.getMonth(), now.getDate(),
    now.getHours(), now.getMinutes() + 1, 0, 0,
  );

  const domIsStar = fields[2] === '*';
  const dowIsStar = fields[4] === '*';
  const domsArr = [...doms];
  const monsArr = [...mons];
  const domMin = domsArr.length ? Math.min(...domsArr) : 1;

  // 提前剪枝：最小月份容不下最小日 → 无解
  const maxDayByMonth = [0, 31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  const possible = monsArr.some((m) => maxDayByMonth[m] >= domMin);
  if (!possible) return '';

  let iters = 0;
  let cursor = new Date(start.getTime());
  const limit = start.getTime() + 366 * 86400000;
  while (cursor.getTime() - start.getTime() < 366 * 86400000 && cursor.getTime() <= limit) {
    const mo = cursor.getMonth() + 1; // chrono month() 是 1-indexed；JS 0-indexed +1
    const d = cursor.getDate();
    const dow = WEEKDAY_FROM_SUNDAY(cursor);
    if (!mons.has(mo)) { cursor = nextDay(cursor); continue; }

    // Vixie cron 日匹配语义（与 JS 原版 cc-status.js:77-92 一致）
    let dayOk;
    if (domIsStar && dowIsStar) dayOk = doms.has(d) || dows.has(dow);
    else if (domIsStar) dayOk = dows.has(dow) && doms.has(d);
    else if (dowIsStar) dayOk = doms.has(d) && dows.has(dow);
    else dayOk = doms.has(d) || dows.has(dow);
    if (!dayOk) { cursor = nextDay(cursor); continue; }

    const y = cursor.getFullYear();
    for (const hr of hrs) {
      for (const mi of mins) {
        const cand = new Date(y, cursor.getMonth(), d, hr, mi, 0, 0);
        if (cand.getTime() < start.getTime()) continue;
        iters += 1;
        if (iters > 500) return '';
        const secs = Math.floor((cand.getTime() - now.getTime()) / 1000);
        if (secs < 60) return '< 1m';
        if (secs < 3600) return `${Math.floor(secs / 60)}m`;
        if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
        const days = Math.floor(secs / 86400);
        return `${days}d ${Math.floor((secs % 86400) / 3600)}h`;
      }
    }
    cursor = nextDay(cursor);
  }
  return '';
}
