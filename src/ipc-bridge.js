// Tauri IPC 封装（架构修订 §2：Channel<Vec<u8>> 流式二进制，无 base64）。
// withGlobalTauri: true → window.__TAURI__.core / event 全局可用，无需 npm 包。

const { invoke, Channel } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

/** 前端调试日志 → Rust → 终端 stdout（无需 DevTools） */
const _debugLogBuffer = [];
export function debugLog(tag, msg) {
  const ts = new Date().toISOString().substring(11, 19);
  const line = `${ts} [${tag}] ${msg}`;
  _debugLogBuffer.push(line);
  if (_debugLogBuffer.length > 200) _debugLogBuffer.shift();
  invoke('debug_log', { tag, msg }).catch(() => {}); // fire-and-forget
}
/** 读取缓存的调试日志 */
export function getDebugLogBuffer() {
  return _debugLogBuffer.join('\n');
}
/** 诊断：将侧边栏完整 DOM 结构写到 Rust 日志 */
export function dumpSidebarToRust(label) {
  const el = document.getElementById('orbital');
  if (!el) return;
  const lines = [`=== ${label} ===`];
  const walk = (node, depth) => {
    const indent = '  '.repeat(depth);
    const tag = node.tagName?.toLowerCase() || '?';
    const cls = (node.className?.toString() || '').substring(0, 60);
    const id = node.id || '';
    const dataId = node.dataset?.id ? node.dataset.id.substring(0, 8) : '';
    const dataPid = node.dataset?.projectId ? node.dataset.projectId.substring(0, 8) : '';
    const cs = window.getComputedStyle(node);
    const borders = [];
    if (cs.borderLeftWidth !== '0px') borders.push(`L=${cs.borderLeftWidth}(${cs.borderLeftColor})`);
    if (cs.borderRightWidth !== '0px') borders.push(`R=${cs.borderRightWidth}`);
    if (cs.borderTopWidth !== '0px') borders.push(`T=${cs.borderTopWidth}(${cs.borderTopColor})`);
    if (cs.borderBottomWidth !== '0px') borders.push(`B=${cs.borderBottomWidth}(${cs.borderBottomColor})`);
    const borderStr = borders.length ? ` BORDER:${borders.join(' ')}` : '';
    let info = `${indent}${tag}`;
    if (id) info += `#${id}`;
    if (cls) info += `.${cls.split(' ').join('.')}`;
    if (dataId) info += `[${dataId}]`;
    if (dataPid) info += `@${dataPid}`;
    info += `${borderStr} h=${cs.height}`;
    if (node.children.length === 0 && node.textContent?.trim()) info += ` "${node.textContent.trim().substring(0, 30)}"`;
    lines.push(info);
    for (const child of node.children) walk(child, depth + 1);
  };
  walk(el, 0);
  // 一次性发送完整 dump
  invoke('debug_log', { tag: 'sidebar-dump', msg: lines.join('\n') }).catch(() => {});
}

// 缓存 TextEncoder 实例，避免每次 ptyWrite 调用都 new
const _textEncoder = new TextEncoder();

/** 规范化 Channel 收到的字节为 Uint8Array（兼容 number[] 与 Uint8Array 两种传输）。 */
function toBytes(chunk) {
  return chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk);
}

// ── P1-13: Backend crash detection ──────────────────────────────────
// Track consecutive IPC failures. After 3, dispatch 'ipc-disconnected'.
// On 2 consecutive successes after disconnect, dispatch 'ipc-reconnected'.
// Hysteresis prevents banner flicker on a flaky backend.

const FAIL_THRESHOLD = 3;
const RECOVER_THRESHOLD = 2;
let _consecutiveFailures = 0;
let _consecutiveRecoveries = 0;

/**
 * Safe wrapper around Tauri invoke(). Counts consecutive failures and
 * dispatches custom DOM events when the backend appears disconnected
 * or recovers. All module-level invoke() calls should go through this.
 */
async function safeInvoke(cmd, args) {
  try {
    const result = await invoke(cmd, args);
    if (_consecutiveFailures >= FAIL_THRESHOLD) {
      _consecutiveRecoveries++;
      if (_consecutiveRecoveries >= RECOVER_THRESHOLD) {
        window.dispatchEvent(new CustomEvent('ipc-reconnected'));
        _consecutiveRecoveries = 0;
      }
    } else {
      _consecutiveRecoveries = 0;
    }
    _consecutiveFailures = 0;
    return result;
  } catch (err) {
    _consecutiveRecoveries = 0;
    _consecutiveFailures++;
    if (_consecutiveFailures === FAIL_THRESHOLD) {
      window.dispatchEvent(new CustomEvent('ipc-disconnected'));
    }
    throw err;
  }
}

/**
 * 创建终端。onData 回调接收 PTY 输出字节（已规范化）。
 * 内部 new Channel()，onmessage → onData；Channel 对象作为 dataChannel 传入 pty_spawn。
 * projectId: 可选，关联的项目 ID。
 * backend: 可选，后端 ID（如 "claude-code"、"opencode"）。
 * agent: 可选，agent 身份（`@name` 的 id）。以该身份**新开一个 Slot**，
 *        而不是往同一个 PTY 里打 `@name` 文本。
 * providerId: 可选，该 Slot 绑定的供应商 id（自选模型）。不给则跟随全局激活供应商。
 * model: 可选，模型覆盖（同一供应商换模型）。
 * 返回 { id, pid, agent, agent_injected, effective_provider, effective_model, provider_fallback }：
 *        agent_injected=false 表示身份**没有**生效；provider_fallback=true 表示请求的
 *        供应商不存在、已回退 —— 两种都必须如实显示，不能假装。
 */
export async function ptySpawn({ cmd, args, cwd, env, label, projectId, backend, agent, providerId, model, cols, rows, onData }) {
  const channel = new Channel();
  channel.onmessage = (chunk) => {
    const bytes = toBytes(chunk);
    onData(bytes);
  };
  return safeInvoke('pty_spawn', { cmd, args, cwd, env, label, projectId, backend: backend || null, agent: agent || null, providerId: providerId || null, model: model || null, cols, rows, dataChannel: channel });
}

/** 手动重启：前端传入新的 onData 回调（旧 Channel 已失效）+ 终端当前尺寸。 */
export async function ptyRestart(id, onData, cols, rows) {
  const channel = new Channel();
  channel.onmessage = (chunk) => {
    const bytes = toBytes(chunk);
    onData(bytes);
  };
  return safeInvoke('pty_restart', { id, cols, rows, dataChannel: channel });
}

export async function ptyKill(id) {
  return safeInvoke('pty_kill', { id });
}

/** data 为 term.onData 给的 string（UTF-8），转字节后传 Rust Vec<u8>。 */
export async function ptyWrite(id, data) {
  const bytes = Array.from(_textEncoder.encode(data));
  return safeInvoke('pty_write', { id, data: bytes });
}

export async function ptyResize(id, cols, rows) {
  return safeInvoke('pty_resize', { id, cols, rows });
}

export async function ptyList() {
  return safeInvoke('pty_list');
}

export async function ptyRename(id, label) {
  return safeInvoke('pty_rename', { id, label });
}

/** Tab 切换时拉取 ring buffer 回放（返回 number[]，前端转 Uint8Array）。 */
export async function ptyReplay(id) {
  const arr = await safeInvoke('pty_replay', { id });
  return toBytes(arr);
}

/** 进程退出事件（低频，Rust 用 app.emit，不走 Channel）。 */
export function onPtyExit(id, callback) {
  return listen(`pty:exit:${id}`, (event) => callback(event.payload));
}

/** 进程自动重启成功事件（Rust auto-restart 成功后 emit，payload 为新 pid）。 */
export function onPtyRestart(id, callback) {
  return listen(`pty:restart:${id}`, (event) => callback(event.payload));
}

// ── P1：会话持久化 ──────────────────────────────────────────────────

/** 以 Rust 侧 slot 为事实源快照并落库（无需前端回传配置）。返回持久化条数。 */
export async function sessionPersist() {
  return safeInvoke('session_persist');
}

/** 启动恢复：返回上次保存的终端配置列表。 */
export async function sessionRestore() {
  return safeInvoke('session_restore');
}

/** 标记指定终端为当前活跃（更新 last_active_at 时间戳）。 */
export async function ptySetActive(id) {
  return safeInvoke('pty_set_active', { id });
}

/** 显式保存（前端构造的配置，兜底用）。 */
export async function sessionSave(slots) {
  return safeInvoke('session_save', { slots });
}

// ── P1：CC Status（skills/hooks/plugins/tasks/agents） ──────────────

/** 读取 CC Status。project_dir 为当前活跃终端 cwd（读 {cwd}/.claude）。 */
export async function ccStatus(projectDir) {
  return safeInvoke('cc_status', { projectDir });
}

export async function ccStatusInvalidate() {
  return safeInvoke('cc_status_invalidate');
}

// ── CC Sessions ──────────────────────────────────────────────────

/** 列出 Claude Code 会话。project_dir 可选过滤（null = 所有项目）。 */
export async function ccSessionsList(projectDir) {
  return safeInvoke('cc_sessions_list', { projectDir: projectDir || null });
}

// ── P1：健康检测 ────────────────────────────────────────────────────

export async function healthCheck() {
  return safeInvoke('health_check');
}

/** 自检开关集合（`{enabled, network}`）。`network` 才允许跑花钱的探测。 */
export async function selfcheckFlags() {
  return safeInvoke('selfcheck_flags', {});
}

/** 自检专用：可写的临时项目目录（只在 ONECODE_SELFCHECK=1 时可用）。 */
export async function selfcheckScratchDir() {
  return safeInvoke('selfcheck_scratch_dir', {});
}

/** 自检开关（后端读 ONECODE_SELFCHECK）。关掉时前端不做任何额外工作。 */
export async function selfcheckEnabled() {
  return safeInvoke('selfcheck_enabled', {});
}

/**
 * 启动期降级（会话库打不开、隔离配置写不进去…）。
 *
 * 这些以前**只躺在日志里** —— 界面上一片正常，用户看到的是"我的终端不见了"。
 * 前端启动时取一次。
 */
export async function startupIssues() {
  return safeInvoke('startup_issues', {});
}

/** 后台健康告警（Rust 每 5s emit，仅有告警项时推送）。 */
export function onHealthReport(callback) {
  return listen('health:report', (event) => callback(event.payload));
}

// ── 配置管理 ──────────────────────────────────────────────────────────

/** 保存应用配置到 ~/.onecode/desktop.json */
export async function saveConfig(config) {
  return safeInvoke('save_config', { config });
}

/** 从 ~/.onecode/desktop.json 加载配置 */
export async function loadConfig() {
  return safeInvoke('load_config');
}

/** 获取 OneCode 配置文件路径（供 agent 直接读写） */
export async function getConfigPath() {
  return safeInvoke('get_config_path');
}

/** 获取配置字段 schema（名称/类型/描述/默认值，供 agent 参考） */
export async function getConfigSchema() {
  return safeInvoke('get_config_schema');
}

/** 监听配置文件外部变更（agent 直接修改后被热加载） */
export function onConfigChanged(callback) {
  return listen('config-changed', () => callback());
}


// ── M1：多供应商 / 模型切换（Provider）────────────────────────────────

/** 列出全部供应商 + 目录元数据（F3 / 芯片渲染用） */
export async function providersList() {
  return safeInvoke('providers_list');
}

/** 两档预置供应商（新增时下拉带出默认 base_url + 型号） */
export async function providersPresets() {
  return safeInvoke('providers_presets');
}

/** 新增供应商（预设或自定义四要素） */
export async function providersAdd(provider) {
  return safeInvoke('providers_add', { provider });
}

/** 编辑供应商（仅覆盖提供的字段） */
export async function providersUpdate(id, updates) {
  return safeInvoke('providers_update', { id, updates });
}

/** 删除供应商（后端强制约束：仅剩 1 家 / 当前使用中不可删） */
export async function providersDelete(id) {
  return safeInvoke('providers_delete', { id });
}

/** 测试供应商连通性（返回 { ok, latency_ms, error }） */
export async function providersTest(id) {
  return safeInvoke('providers_test', { id });
}

/** 手动切换（F2：写 desktop.json + providers.json → 重启会话） */
export async function providersSwitch(providerId) {
  return safeInvoke('providers_switch', { providerId });
}

/** 切档后刷新指定 slot 的 env（pty.restart 复用旧 env，必须先刷新） */
export async function ptyRefreshEnv(id) {
  return safeInvoke('pty_refresh_env', { id });
}

/** 切换成功事件（payload: { from, to, reason }） */
export function onProviderSwitched(callback) {
  return listen('provider-switched', (event) => callback(event.payload));
}

/** 供应商目录变更事件（增删改后由后端 emit） */
export function onProvidersChanged(callback) {
  return listen('providers-changed', () => callback());
}

// ── 项目管理 ──────────────────────────────────────────────────────────

/** 保存项目元数据到 ~/.onecode/projects/<name>.json */
export async function saveProject({ name, dir, description, backend }) {
  return safeInvoke('save_project', { project: { name, dir, description, backend: backend || null } });
}

/** 列出所有已保存的项目 */
export async function listProjects() {
  return safeInvoke('list_projects');
}

/** 删除项目元数据（不删除目录和关联终端） */
export async function deleteProject(name) {
  return safeInvoke('delete_project', { name });
}

// ── 托盘 / 应用生命周期事件 ─────────────────────────────────────────

/** 托盘「新建终端」点击。 */
export function onTrayNewTerminal(callback) {
  return listen('tray:new-terminal', () => callback());
}

/** 应用即将退出（托盘「退出」触发）——前端在此做最终会话保存。 */
export function onAppBeforeQuit(callback) {
  return listen('app:before-quit', () => callback());
}

/** 应用菜单栏事件（Rust menu.rs emit `menu:<id>`，P0-4）。 */
export function onMenuEvent(id, callback) {
  return listen(`menu:${id}`, () => callback());
}

// ── 系统信息 ──────────────────────────────────────────────────────────

/** 获取当前用户的 home 目录（跨平台，Rust 侧读取 HOME/USERPROFILE）。 */
export async function getHomeDir() {
  return safeInvoke('get_home_dir');
}

// ── Backend 管理 ──────────────────────────────────────────────────────

/** 列出所有可用后端及其安装状态。返回 BackendInfo[]。 */
export async function listBackends(cwd) {
  // 带上 cwd：项目层 `<cwd>/.onecode/engines.json` 里的引擎也要列出来
  // （不传 = 只看内置 + 全局，行为与以前一致）
  return safeInvoke('list_backends', { cwd: cwd || null });
}

// ── Setup Wizard ──────────────────────────────────────────────────

/** 是否首次启动（wizard 未完成） */
export async function isFirstRun() {
  return safeInvoke('is_first_run');
}

/** 检测环境依赖（Claude Code / Node.js / Git） */
export async function checkEnvironment() {
  return safeInvoke('check_environment');
}

/** 保存 Wizard 配置（API Key / Base URL / Model / Backend + 标记 wizard_completed） */
export async function saveWizardConfig({ apiKey, baseUrl, model, backend }) {
  return safeInvoke('save_wizard_config', {
    config: { api_key: apiKey, base_url: baseUrl, model, backend: backend || 'claude-code' }
  });
}

// ── 文件浏览器 ──────────────────────────────────────────────

/** 列出目录内容（返回 {path, entries[]}）。 */
export async function fsListDir(path) {
  return safeInvoke('fs_list_dir', { path });
}

/** 读取文件内容用于预览（返回 {name, path, fileType, text, dataBase64, size}）。 */
export async function fsReadFile(path) {
  return safeInvoke('fs_read_file', { path });
}

// ── 外链 / 编辑器打开（optimization-005 / 007）──────────────────────

/** 用系统默认浏览器打开外链（Ctrl/Cmd+Click 链接、Markdown 预览链接）。 */
export async function openExternal(url) {
  return safeInvoke('open_external', { url });
}

/** 用本地 VS Code 打开目录/文件（"用 VS Code 打开"按钮）。 */
export async function openInVscode(path) {
  return safeInvoke('open_in_vscode', { path });
}

// ── 产出与证据化验收（目标 ④）────────────────────────────────────────
//
// 注意：这三个命令**只由人触发**。模型不能调用它们，所以它无法把自己的产出
// 标成"已验收" —— 验收状态是人的动作，不是 agent 的自我评价。

/** 扫描一个工作区的产出记录（真文件核对 + 证据分级 + 本机量到的 git 改动）。 */
export async function deliverablesScan(cwd) {
  return safeInvoke('deliverables_scan', { cwd });
}

/** 人显式改验收状态：draft | pending | accepted | rejected。 */
export async function deliverableSetStatus(cwd, id, status, note) {
  return safeInvoke('deliverable_set_status', { cwd, id, status, note: note || null });
}

/**
 * 产出审查门：弹对话框把事实/证据/问题摊开，人点「接受 / 驳回」。
 * 返回 'accepted' | 'rejected' | null（人取消）。
 */
export async function deliverableReview(cwd, id) {
  return safeInvoke('deliverable_review', { cwd, id: id || null });
}

/**
 * 验证证据：把 agent 声明的 exec 证据**跑一遍**（人确认后才跑），
 * 结果写回产出记录。返回 Verification | null（人取消）。
 *
 * ⚠️ 命令由 agent 写在产出记录里，以你的 OS 用户身份、**无沙箱**执行 ——
 * 所以 Rust 侧会先把命令原样列出来让人确认。
 */
export async function deliverableVerify(cwd, id) {
  return safeInvoke('deliverable_verify', { cwd, id: id || null });
}

// ── 任务板（目标 ⑤ 的第一块）────────────────────────────────────────
//
// 治理边界：这些命令**只由人触发**。agent 能写任务定义（它有文件写权限），
// 但**不能裁决状态** —— 它自己写的 `done` 只会被标成"自报，未经人裁决"。

/** 读任务板（任务定义 + 人的裁决 + 现场查出来的产出关联）。 */
export async function taskList(cwd) {
  return safeInvoke('task_list', { cwd });
}

/** 建一条任务（人触发），返回新任务 id。 */
export async function taskCreate(cwd, title, owner, note, providerId, model) {
  return safeInvoke('task_create', {
    cwd,
    title,
    owner: owner || null,
    note: note || null,
    providerId: providerId || null,
    model: model || null,
  });
}

/** 人的裁决：todo | doing | blocked | review | done | dropped。 */
export async function taskSetStatus(cwd, id, status, note) {
  return safeInvoke('task_set_status', { cwd, id, status, note: note || null });
}

/** 改任务的负责人 / 关联产出。 */
export async function taskAssign(cwd, id, owner, deliverables, providerId, model) {
  return safeInvoke('task_assign', {
    cwd,
    id,
    owner,
    deliverables: deliverables || null,
    providerId: providerId || null,
    model: model || null,
  });
}

/** 把任务板摆成只读对话框。 */
export async function taskBoard(cwd) {
  return safeInvoke('task_board', { cwd });
}

/** 任务裁决门：人点「完成 / 退回重做」。返回 'done' | 'doing' | null。 */
export async function taskReview(cwd, id) {
  return safeInvoke('task_review', { cwd, id: id || null });
}

// ── 账本（目标 ⑤ 的第四块）──────────────────────────────────────────
//
// 只报可测量的事实：产出侧来自我们自己的记录，机时侧（token）来自本机 transcript。
// **金额不算** —— transcript 里没有成本字段（实测），算金额需要价格表。

/**
 * 读账本（任务/产出/验收/证据 + token 用量）。
 *
 * `full = true` 会做**全库完整扫描**（本机实测约 15 秒，瓶颈是文件 I/O）。
 * 默认 false：只算本项目（实测 246ms）+ 全库只数文件。
 */
export async function ledgerScan(cwd, full) {
  return safeInvoke('ledger_scan', { cwd, full: !!full });
}

/** 把账本摆成对话框。`full` 同上（慢路径会明确显示"本次未算"与否）。 */
export async function ledgerShow(cwd, full) {
  return safeInvoke('ledger_show', { cwd, full: !!full });
}

// ── 员工模型花名册（目标 ②：按员工自选模型）──────────────────────────
//
// 绑定与实际**分开报**：只给绑定，你不知道实际跑的是别的；
// 只给实际，你不知道自己设过什么。两者并列，不一致才看得见。

/** 花名册（每个员工一行：绑定 + 实际解析出来的路线，含 failover 结果）。 */
export async function rosterList() {
  return safeInvoke('roster_list', {});
}

/** 给某个员工绑定路线。model 留空 = 用该供应商自己的默认模型。 */
export async function rosterSet(agentId, providerId, model) {
  return safeInvoke('roster_set', {
    agentId,
    providerId,
    model: model || null,
  });
}

/** 解除绑定（回到全局激活供应商）。 */
export async function rosterClear(agentId) {
  return safeInvoke('roster_clear', { agentId });
}

/** 花名册对话框。 */
export async function rosterShow() {
  return safeInvoke('roster_show', {});
}

// ── 引擎定义（目标 ①：可换引擎）──────────────────────────────────────
//
// 内置 7 个 + `~/.onecode/engines.json` 里的用户定义。加引擎不需要重编译。

/** 引擎列表对话框（来源标记 / 能力 / 坏定义的问题清单）。 */
export async function enginesShow(cwd) {
  // 同上：项目层引擎要能被看见（"悄悄多了一个引擎"是不行的）
  return safeInvoke('engines_show', { cwd: cwd || null });
}

/** 引擎配置示例（文件路径 + 可直接抄的 JSON + 规则说明）。 */
export async function enginesExampleShow() {
  return safeInvoke('engines_example_show', {});
}

// ── 引擎能力协商（目标 ①）──────────────────────────────────────────
//
// 问引擎自己有什么能力，而不是靠我们在定义里手写。
// 实测：`-p ok --output-format stream-json --verbose` 会在**推理之前**先吐
// system/init，我们读到就杀进程 —— 所以探测不花 token。

/** 探测一个引擎的能力（返回结构化的 init 报告）。 */
export async function engineProbe(engineId, cwd) {
  return safeInvoke('engine_probe', { engineId, cwd });
}

/** 探测并弹对话框显示结果。 */
export async function engineProbeShow(engineId, cwd) {
  return safeInvoke('engine_probe_show', { engineId, cwd });
}

// ── 结构化跑一次（目标 ① 的非 PTY 路径 + ⑤ 账本的金额）────────────────
//
// 不走 TUI：`-p <prompt> --output-format stream-json`，输出是结构化事件，
// 结束时引擎给出 result.total_cost_usd 与 modelUsage（含它自报的上下文窗口）。
// 每次运行都会被记进 <cwd>/.onecode/runs.jsonl，账本据此显示真实金额。

/** 结构化跑一次。返回 RunReport（含金额/用量/窗口）。 */
export async function engineRun(cwd, prompt, agent, taskId) {
  return safeInvoke('engine_run', {
    cwd,
    prompt,
    agent: agent || null,
    taskId: taskId || null,
  });
}

/** 结构化跑一次并弹对话框显示结果。 */
export async function engineRunShow(cwd, prompt, agent, taskId) {
  return safeInvoke('engine_run_show', {
    cwd,
    prompt,
    agent: agent || null,
    taskId: taskId || null,
  });
}

// ── 结构化常驻 Slot（目标 ①：可换引擎不该要求引擎长得像终端）────────
//
// 与终端槽位的区别：走结构化协议，每轮有**结算**（金额/用量），多轮靠固定
// `session id` 接续。合计金额由后端算好（视图字段），前端不重算。

/** 开一个结构化槽位。 */
export async function structuredOpen(cwd, agent, providerId, model) {
  return safeInvoke('structured_open', {
    cwd,
    agent: agent || null,
    providerId: providerId || null,
    model: model || null,
  });
}

/** 说一句：跑一轮并把结果记进记录。 */
export async function structuredSend(cwd, id, prompt) {
  return safeInvoke('structured_send', { cwd, id, prompt });
}

/** 读一个槽位。 */
export async function structuredState(cwd, id) {
  return safeInvoke('structured_state', { cwd, id });
}

/** 列出本项目里的结构化槽位。 */
export async function structuredList(cwd) {
  return safeInvoke('structured_list', { cwd });
}
