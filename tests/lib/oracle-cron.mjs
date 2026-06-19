// 从 onecode/agent-runtime/gateway/cc-status.js 抽取原版 cronNext 作为 oracle。
// 原版通过 brace-matching 完整提取（含内部 expand），用 new Function 隔离求值，
// 不 require 模块（避免 ./config / node-pty 依赖），从而可注入可控 now。
//
// 用法：
//   const cronNextOrig = loadOracleCronNext();            // 用真实 Date
//   const cronNextAt = loadOracleCronNext(now);           // 注入固定 now（Determinism）

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const CC_STATUS_JS = join(__dirname, '..', '..', '..', 'onecode', 'agent-runtime', 'gateway', 'cc-status.js');

function extractCronNextSource(src) {
  const start = src.indexOf('function cronNext');
  if (start < 0) throw new Error('cronNext not found in cc-status.js');
  const openBrace = src.indexOf('{', start);
  let depth = 0;
  let end = -1;
  for (let i = openBrace; i < src.length; i++) {
    const ch = src[i];
    if (ch === '{') depth++;
    else if (ch === '}') {
      depth--;
      if (depth === 0) { end = i; break; }
    }
  }
  if (end < 0) throw new Error('cronNext braces unbalanced');
  return src.slice(start, end + 1);
}

export function loadOracleCronNext(fixedNow) {
  const src = readFileSync(CC_STATUS_JS, 'utf8');
  const fnSrc = extractCronNextSource(src);
  // 注入 Date：若提供 fixedNow，用 Proxy 让 `new Date()` 返回固定时刻（无参），
  // 其余调用（带参拷贝、getTime 等）走真实 Date。
  let DateImpl = globalThis.Date;
  if (fixedNow != null) {
    const real = globalThis.Date;
    DateImpl = new Proxy(real, {
      construct(target, args) {
        if (args.length === 0) return new real(fixedNow);
        return new target(...args);
      },
      apply(target, thisArg, args) {
        if (args.length === 0) return new real(fixedNow);
        return target.apply(thisArg, args);
      },
      get(target, prop) {
        const v = target[prop];
        return typeof v === 'function' ? v.bind(target) : v;
      },
    });
  }
  // eslint-disable-next-line no-new-func
  const factory = new Function('Date', `${fnSrc}; return cronNext;`);
  return factory(DateImpl);
}

export const ORACLE_PATH = CC_STATUS_JS;
