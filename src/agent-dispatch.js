// 员工派发与点名的**共享实现**。
//
// 为什么单独成模块：侧栏（agents-list）、光球面板（orbital）、命令面板（palette）
// 三处都能点 agent。原先这三处**各自**写了一份"往 PTY 打 `@id `"——
// 于是把"点击 = 派发"改了一处、漏了三处，正是 CLAUDE.md 规则 7 的典型失效模式
// （改了判断逻辑，调用点各抄一份，编译器看不出来，测试也未必覆盖）。
//
// 现在语义只有两份实现，且语义边界清晰：
//   - `dispatchAgent`     → **点击**：以该 agent 为身份**新开一个 Slot**；
//   - `insertAgentMention` → **⌥/Ctrl/Cmd + 点击**：在当前会话里**点名**（插入 `@id `）。
// 另外 `terminal/mention.js` 的插入是"补全用户正在输入的 @"，机制不同，不并入这里。

/**
 * 派发：以某个 agent 为身份新开一个 Slot。
 *
 * 需要"当前项目"上下文（终端本来就按项目组织），所以以活跃终端的
 * projectId / cwd / backend / 路由为模板；拿不到时**如实提示**，而不是悄悄退回打字。
 *
 * @param {object}   o
 * @param {object}   o.tabManager  需要 `createTab` / `activeId` / `tabs`
 * @param {string}   o.agentId     agent 的 id（@name）
 * @param {Array}    [o.agents]    用于取显示名与 icon（缺省则回退 `@id`）
 * @param {Function} [o.onNotice]  人可见的提示（各表面用自己的 toast）
 * @returns {Promise<boolean>} true = 已创建 Slot
 */
export async function dispatchAgent({ tabManager, agentId, agents, onNotice }) {
  const notice = typeof onNotice === 'function' ? onNotice : (m) => console.warn('[dispatch]', m);

  if (!tabManager || typeof tabManager.createTab !== 'function') {
    notice('派发不可用（终端管理器未就绪）');
    return false;
  }

  const active = tabManager.activeId ? tabManager.tabs.get(tabManager.activeId) : null;
  const usable = active && !active.isError;
  const projectId = usable ? active.projectId : null;
  if (!projectId) {
    notice('请先打开一个项目 —— 派发需要项目上下文');
    return false;
  }

  const list = Array.isArray(agents) ? agents : [];
  const def = list.find((a) => a.id === agentId);
  const label = def ? `${def.icon ? def.icon + ' ' : ''}${def.name}` : `@${agentId}`;

  try {
    await tabManager.createTab({
      label,
      projectId,
      cwd: usable ? active.cwd : undefined,
      backend: usable ? active.backend : null,
      agent: agentId,
      // 继承当前项目的路由：新员工跑在与项目相同的供应商/模型上。
      // （员工级模型要等 frontmatter 的 `model:` 语义定下来再启用 —— 直接把
      //   Claude 型号名塞给第三方供应商，比不设更危险。）
      providerId: usable ? active.providerId || null : null,
      model: usable ? active.model || null : null,
    });
    return true;
  } catch (e) {
    notice(`派发失败：${e && e.message ? e.message : e}`);
    return false;
  }
}

/**
 * 点名：把 `@id ` 插入当前会话的终端（不新建 Slot）。
 *
 * @param {object}   o
 * @param {object}   o.tabManager
 * @param {string}   o.agentId
 * @param {Function} [o.ptyWrite]  兜底写入（mention 控制器不可用时）
 * @param {Function} [o.onNotice]
 * @returns {boolean} true = 已写入
 */
/**
 * 往**活跃 Slot 的 PTY** 写入一段文本 —— "有意写入"的**唯一实现**。
 *
 * 为什么单独抽出来：`insertAgentMention`（点名）与 `sendTaskToSlot`（派活）本质是
 * 同一件事（都是"人主动往当前会话塞一段字"）。各写一份的话，写入路径
 * （mention.sendInput 优先、ptyWrite 兜底）就会漂移 —— 老问题。
 *
 * 注意：**不追加回车**。写入只是填进输入框，由人按回车发送 ——
 * 自动发送等于让人一按就触发一段自己没审过的指令。
 *
 * @returns {boolean} true = 已写入
 */
export function writeToActiveSlot({ tabManager, text, ptyWrite, onNotice }) {
  const notice = typeof onNotice === 'function' ? onNotice : (m) => console.warn('[slot-write]', m);

  if (!tabManager || !tabManager.activeId) {
    notice('请先创建一个终端');
    return false;
  }
  const activeId = tabManager.activeId;
  const st = tabManager.tabs.get(activeId);
  if (!st || st.isError) {
    notice('请先创建一个终端');
    return false;
  }

  // 路径 1（首选）：复用 MentionController.sendInput —— 与手动敲 @ 选中后同一条写入路径
  if (st.mention && typeof st.mention.sendInput === 'function') {
    if (st.mention.active) st.mention.hide();
    try {
      st.mention.sendInput(text);
      return true;
    } catch (e) {
      console.warn('[slot-write] sendInput failed, falling back to ptyWrite:', e);
    }
  }
  // 路径 2（兜底）
  if (typeof ptyWrite === 'function') {
    Promise.resolve(ptyWrite(activeId, text)).catch((e) => {
      console.warn('[slot-write] ptyWrite failed:', e);
    });
    return true;
  }
  return false;
}

export function insertAgentMention({ tabManager, agentId, ptyWrite, onNotice }) {
  const notice = typeof onNotice === 'function' ? onNotice : (m) => console.warn('[mention]', m);
  return writeToActiveSlot({
    tabManager,
    text: `@${agentId} `,
    ptyWrite,
    onNotice: notice,
  });
}

/**
 * 派活：把一条任务的正文填进**当前活跃 Slot**（就是正在对话的那个员工）。
 *
 * 不写 `@owner` 前缀：我们本来就是在这个员工的会话里，自己 @ 自己没有意义。
 * 也不代按回车 —— 人看过再发。
 *
 * @param {object} o
 * @param {object} o.task 任务对象（来自 Rust `task_list`）
 * @returns {boolean} true = 已填入
 */
export function sendTaskToSlot({ tabManager, task, ptyWrite, onNotice }) {
  const notice = typeof onNotice === 'function' ? onNotice : (m) => console.warn('[task]', m);
  if (!task || !task.id) {
    notice('没有可派发的任务');
    return false;
  }
  const lines = [`任务 ${task.id}：${task.title}`];
  if (task.note) lines.push(task.note);
  lines.push('');
  lines.push('做完后：把交付写成产出记录，再把本任务状态改成 review 等我验收。');
  return writeToActiveSlot({
    tabManager,
    text: `${lines.join('\n')}\n`,
    ptyWrite,
    onNotice: notice,
  });
}

/** 点击是否应走"点名"而不是"派发"（⌥ / Ctrl / Cmd）。 */
export function isMentionModifier(event) {
  return !!(event && (event.altKey || event.ctrlKey || event.metaKey));
}
