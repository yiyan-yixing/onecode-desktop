// 自检：在**真实 WebView 里**对**真实 Rust 命令**逐条校验返回形状。
//
// 为什么需要它：`test:boot` 用的是桩（`window.__TAURI__` 被换掉），所以它证明的是
// "前端逻辑对"，证明不了"真命令真的回答了、形状真的对"。而 ④⑤ 那一批命令
// ——`deliverables_scan` / `task_list` / `ledger_scan` / `roster_list` /
// `engines_problems` / `startup_issues`——**真前端从来没有调用过**（它们只在
// 命令面板或对话框里按需触发，没人点就永远不走）。
//
// 于是：由 `ONECODE_SELFCHECK=1` 门控，启动时跑一遍，结果经 `debug_log` 落到进程日志。
// 关掉时一行开销都没有。

/**
 * @param {object} ipc  ipc-bridge 模块
 * @param {object} ctx  `{ cwd, hasProject }` —— 写类命令需要真实项目目录，否则跳过
 * @returns {Promise<Array<{name:string, status:'pass'|'fail'|'skip', why:string}>>}
 */
export async function runSelfCheck(ipc, ctx = {}) {
  let cwd = ctx.cwd || '';
  // 写类命令需要**可写**的项目目录：
  // - 优先用真实活动项目；
  // - 没有就向后端要一个**临时目录**（碰不到用户任何真实数据）；
  // - 连临时目录都拿不到，才显式跳过 —— 跳过 ≠ 通过。
  let canWrite = !!cwd;
  let scratch = false;
  if (!canWrite) {
    try {
      cwd = await ipc.selfcheckScratchDir();
      canWrite = !!cwd;
      scratch = true;
    } catch (e) {
      canWrite = false;
    }
  }
  // 只读用的"项目 cwd"：配置里的 default_cwd（**只读**，不往里写任何东西）。
  // 写类探测一律走 scratch 目录 —— 自检绝不能污染用户的真实项目。
  let projectCwd = '';
  let defaultBackend = '';
  try {
    const cfg0 = await ipc.loadConfig();
    projectCwd = (cfg0 && cfg0.default_cwd) || '';
    defaultBackend = (cfg0 && cfg0.default_backend) || '';
  } catch (e) {
    /* 读不到就当没有项目 */
  }

  const checks = [];
  const add = (name, ok, why) => checks.push({ name, status: ok ? 'pass' : 'fail', why: why || '' });
  const skip = (name, why) => checks.push({ name, status: 'skip', why: why });

  // 每条：调用 → 断言**形状**（不只是"没抛错"）
  const probe = async (name, fn, assert) => {
    try {
      const v = await fn();
      const [ok, why] = assert(v);
      add(name, ok, why);
    } catch (e) {
      add(name, false, '抛错：' + (e && e.message ? e.message : e));
    }
  };

  await probe('startup_issues', () => ipc.startupIssues(), (v) => [
    Array.isArray(v),
    Array.isArray(v) ? `${v.length} 条` : '不是数组',
  ]);
  await probe('load_config', () => ipc.loadConfig(), (v) => [
    !!(v && typeof v.default_backend === 'string'),
    v && v.default_backend ? `backend=${v.default_backend}` : '缺 default_backend',
  ]);
  await probe('providers_list', () => ipc.providersList(), (v) => [
    !!(v && Array.isArray(v.providers) && v.usable && typeof v.usable === 'object'),
    v ? `providers=${(v.providers || []).length} usable=${Object.keys(v.usable || {}).length}` : '空',
  ]);
  // 带上**活动项目**的 cwd（只读）：项目层 `<cwd>/.onecode/engines.json` 里的
  // 引擎因此能被看见。不传 cwd 就只看内置 + 全局。
  await probe('list_backends', () => ipc.listBackends(projectCwd || undefined), (v) => [
    Array.isArray(v) && v.every((b) => b && typeof b.id === 'string' && typeof b.cmd === 'string'),
    Array.isArray(v)
      ? `${v.length} 个引擎${(v || []).some((b) => b.source === 'Project') ? '（含项目层）' : ''}`
      : '不是数组',
  ]);
  // 注：这里原本还探测 `engines_problems` / `engines_config_path` —— 自检第一次
  // 真跑就发现它们是**注册了但没有任何前端包装**的死命令（`ipc.enginesProblems` 未定义）。
  // 它们的功能已被 `engines_show` / `engines_example_show` 覆盖，于是删掉了命令本身。
  await probe('roster_list', () => ipc.rosterList(), (v) => [
    Array.isArray(v),
    Array.isArray(v) ? `${v.length} 个员工绑定` : '不是数组',
  ]);
  if (!canWrite) skip('deliverables_scan', '跳过：没有可写目录');
  else await probe('deliverables_scan', () => ipc.deliverablesScan(cwd), (v) => [
    !!(v && Array.isArray(v.deliverables) && v.git && typeof v.git.is_repo === 'boolean'),
    v ? `产出=${(v.deliverables || []).length} git=${v.git && v.git.is_repo}` : '空',
  ]);
  if (!canWrite) skip('task_list', '跳过：没有可写目录');
  else await probe('task_list', () => ipc.taskList(cwd), (v) => [
    !!(v && Array.isArray(v.tasks) && typeof v.unreadable === 'number'),
    v ? `任务=${(v.tasks || []).length}` : '空',
  ]);

  // ── 任务板**真实往返**（④⑤ 的核心命令，真前端此前从没跑过）──
  // 只写临时目录；任务是"声明 + 人的裁决"两份文件，往返要能看出裁决生效。
  if (!canWrite) {
    skip('task_roundtrip', '跳过：没有可写目录');
  } else {
    const title = `自检任务 ${Date.now()}`;
    let id = '';
    try {
      id = await ipc.taskCreate(cwd, title, 'dev', '自检用', 'glm', 'glm-4-plus');
      const board = await ipc.taskList(cwd);
      const t = (board.tasks || []).find((x) => x.id === id);
      add('task_roundtrip', !!(t && t.title === title), t ? `建出 ${id}` : '建了但列表里没有');
      if (t) {
        add('task_model_persisted', t.provider_id === 'glm' && t.model === 'glm-4-plus',
            `任务级模型 ${t.provider_id}/${t.model}`);
        add('task_starts_as_todo', t.status === 'todo', `初始状态 ${t.status}`);
      }
      // 人的裁决 → 生效状态要以人为准
      await ipc.taskSetStatus(cwd, id, 'review', '自检');
      const b2 = await ipc.taskList(cwd);
      const t2 = (b2.tasks || []).find((x) => x.id === id);
      add('task_human_ruling_wins',
          !!(t2 && t2.status === 'review' && t2.status_source === 'human'),
          t2 ? `状态 ${t2.status}（来源 ${t2.status_source}）` : '找不到');
    } catch (e) {
      add('task_roundtrip', false, '抛错：' + (e && e.message ? e.message : e));
    }
  }
  if (!canWrite) skip('ledger_scan', '跳过：没有可写目录');
  else await probe('ledger_scan', () => ipc.ledgerScan(cwd), (v) => [
    !!(v && typeof v.cost_usd_total === 'number' && Array.isArray(v.cost_by_task)),
    v ? `合计金额=${v.cost_usd_total} 运行=${v.runs_total}` : '空',
  ]);
  // 结构化槽位：**必须**带后端算好的合计（漏了就是"合计 $0"这种静默错）
  if (canWrite) {
    await probe('structured_open', () => ipc.structuredOpen(cwd, null, null, null), (v) => [
      !!(v && v.id && typeof v.total_cost_usd === 'number' && Array.isArray(v.turns)
         && typeof v.session_mismatch === 'boolean'),
      v ? `id=${String(v.id).slice(0, 8)} turns=${(v.turns || []).length}` : '空',
    ]);
    await probe('structured_list', () => ipc.structuredList(cwd), (v) => [
      Array.isArray(v) && v.every((x) => typeof x.total_cost_usd === 'number'),
      Array.isArray(v) ? `${v.length} 个槽位` : '不是数组',
    ]);
  } else {
    skip('structured_open', '跳过：没有可写目录');
    skip('structured_list', '跳过：没有可写目录');
  }
  // ★ ACP 端到端：**只在当前引擎确实是 ACP 协议时**才跑
  //（本机没有真实 ACP agent，所以通常走 SKIP；有则跑真链路）
  {
    let acpEngine = null;
    try {
      const backs = await ipc.listBackends(projectCwd || undefined);
      acpEngine = (backs || []).find(
        (b) => b.id === defaultBackend && String(b.protocol || '').toLowerCase() === 'acp'
      );
    } catch (e) {
      /* 读不到就当没有 */
    }
    if (!acpEngine) {
      skip('acp_turn', '跳过：当前引擎不是 ACP 协议（本机无真实 ACP agent）');
    } else if (!canWrite) {
      skip('acp_turn', '跳过：没有可写目录');
    } else {
      try {
        const slot = await ipc.structuredOpen(cwd, null, null, null);
        const after = await ipc.structuredSend(cwd, slot.id, '你好');
        const last = (after.turns || [])[after.turns.length - 1] || {};
        add('acp_turn', !!last.reply, `回复=${JSON.stringify(String(last.reply).slice(0, 30))}`);
        add('acp_stop_reason', String(last.extra_meta || '').includes('stop=end_turn'),
            String(last.extra_meta || ''));
        add('acp_protocol_tag', String(last.extra_meta || '').startsWith('ACP'),
            '协议要标出来，别与 print 混为一谈');
      } catch (e) {
        add('acp_turn', false, '抛错：' + (e && e.message ? e.message : e));
      }
    }
  }

  // ★ print 协议的真实一轮（`structured_send` → 引擎 → 账本 → 熔断）
  // 它会**真的调用模型**，所以要花钱 —— 必须显式打开才跑（默认跳过）。
  if (!ctx.network) {
    skip('structured_print_turn', '跳过：会真的调用模型（ONECODE_SELFCHECK_NETWORK=1 才跑）');
  } else if (!canWrite) {
    skip('structured_print_turn', '跳过：没有可写目录');
  } else {
    try {
      const slot = await ipc.structuredOpen(cwd, null, null, null);
      const after = await ipc.structuredSend(cwd, slot.id, '只回复 ok');
      const last = (after.turns || [])[after.turns.length - 1];
      add('structured_print_turn', !!last, '这一轮要被记下来（成功或失败都算）');
      if (last) {
        // 关键不变量：**失败也要留痕**（"失败的花费同样是花费"）
        add('print_turn_honest',
            last.ok ? !!last.reply : !!String(last.error || '').trim(),
            last.ok ? `成功：${JSON.stringify(String(last.reply).slice(0, 20))}`
                    : `失败但如实记了原因：${JSON.stringify(String(last.error).slice(0, 60))}`);
        add('print_turn_meta', String(last.extra_meta || '').startsWith('print'),
            String(last.extra_meta || ''));
      }
      // 账本要能看到这次运行（哪怕花了 0 元、哪怕失败）
      const led = await ipc.ledgerScan(cwd);
      add('print_turn_in_ledger', (led.runs_total || 0) >= 1,
          `运行记录 ${led.runs_total} 条，金额 $${led.cost_usd_total}`);
    } catch (e) {
      add('structured_print_turn', false, '抛错：' + (e && e.message ? e.message : e));
    }
  }

  await probe('cc_sessions_list', () => ipc.ccSessionsList(null), (v) => [
    Array.isArray(v),
    Array.isArray(v) ? `${v.length} 条会话` : '不是数组',
  ]);

  if (canWrite && scratch) {
    ipc.debugLog('selfcheck', `（读写类命令跑在临时目录：${cwd}）`);
  }
  const failed = checks.filter((c) => c.status === 'fail');
  const skipped = checks.filter((c) => c.status === 'skip');
  const ran = checks.length - skipped.length;
  ipc.debugLog(
    'selfcheck',
    `SELFCHECK ${failed.length ? 'FAIL' : 'PASS'} ${ran - failed.length}/${ran}` +
      (skipped.length ? `（跳过 ${skipped.length}）` : '')
  );
  for (const c of checks) {
    const tag = c.status === 'pass' ? 'PASS' : c.status === 'fail' ? 'FAIL' : 'SKIP';
    ipc.debugLog('selfcheck', `${tag} ${c.name.padEnd(20)} ${c.why}`);
  }
  return checks;
}
