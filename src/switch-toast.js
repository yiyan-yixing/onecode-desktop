// 切换提示的文案 —— 纯函数、无 DOM/IPC 依赖（所以能直接单测）。
//
// 为什么值得单独一个模块：熔断触发的**自动切换**如果显示成"已切换到 X"，
// 就与用户自己点的完全一样 —— 而它会**重启你的终端**。
// 自动发生的事必须看起来就是自动发生的。

/**
 * @param {{from?:string,to?:string,reason?:string}} payload  `provider-switched` 事件载荷
 * @param {(id:string)=>string} [nameOf]  把 id 换成显示名（拿不到就退回 id）
 * @returns {{text:string, kind:'ok'|'warn'}}
 */
export function switchToast(payload, nameOf) {
  const to = (payload && payload.to) || '';
  const name = (typeof nameOf === 'function' ? nameOf(to) : '') || to;
  if (payload && payload.reason === 'consecutive_failure') {
    const from = payload.from && payload.from !== '<none>' ? payload.from : '(原供应商)';
    return { text: `${from} 连续失败，已自动切换到 ${name}（终端已重启）`, kind: 'warn' };
  }
  return { text: `已切换到 ${name}`, kind: 'ok' };
}
