# CLI/terminal agent seams — first-hand audit for a multi-slot dispatch console

Research date **2026-10-14**. Every claim below is from primary docs or repo source unless marked *(blog)* or *(unverified)*. Versions move fast; the Codex protocol and Claude Code hook list are read at `main` on this date.

---

## Per-terminal seam

### Claude Code CLI
The seam is a **first-class, addressable session** run by a process happy to be a *turn engine* rather than an app. `claude -p --output-format stream-json --input-format stream-json` gives a long-lived NDJSON duplex: push user messages in, receive assistant/tool/result events out, steer with `--resume <uuid>`, `--fork-session`, `--session-id <uuid>`. Sessions land as JSONL under `~/.claude/projects/<slug>/<session-id>.jsonl`, but the docs say the line format is **internal and changes between versions** — the sanctioned reads are `/export`, the SDK, or the `transcript_path` handed to hooks. Second seam: a real 6-mode permission ladder (`default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`) where "always allow" persists a *scoped rule*, not a blanket yes ([CLI reference](https://code.claude.com/docs/en/cli-reference), [sessions](https://code.claude.com/docs/en/sessions), [permissions](https://code.claude.com/docs/en/permissions)).

**可整合的一件东西：** NDJSON duplex session (`--input-format stream-json` + `--output-format stream-json` + `--resume/--fork-session`) as the per-slot transport, which gives a console a *turn-addressable* slot instead of a PTY screen-scrape.

### OpenAI Codex CLI / Codex Harness
The seam is that Codex **ships its whole GUI protocol as a service and lets you be the UI**. `codex app-server --listen <url>` speaks JSON-RPC 2.0 over `stdio://` (default), `unix://`, `ws://IP:PORT`, or `off`; the full schema is published as JSON Schema under `codex-rs/app-server-protocol/schema/json/`: ~100 client→server methods (`thread/start|resume|fork`, `turn/start|steer|interrupt`, `model/list`, `permissionProfile/list`, `skills/list`, `hooks/list`, `plugin/*`, `mcpServer/*`), ~70 server→client notifications, and 10 **server→client requests** for approvals and custom tools. `item/tool/call` is invertible: the *server asks the host* to run a tool the host owns. Approvals are typed (`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `item/permissions/requestApproval`) and can be routed to an LLM reviewer (`ApprovalsReviewer: user | auto_review | guardian_subagent`). Apache-2.0, Rust ([README](https://raw.githubusercontent.com/openai/codex/main/codex-rs/app-server/README.md), [schemas](https://github.com/openai/codex/tree/main/codex-rs/app-server-protocol/schema/json), [App Server post](https://openai.com/index/unlocking-the-codex-harness/)).

**可整合的一件东西：** `item/tool/call` — the host-owned tool round-trip, which lets the console expose its own Rust capabilities (scheduler, other slots, cost meter) to any Codex thread without MCP.

### OpenCode
The seam is a **real headless HTTP server with a published OpenAPI 3.1 spec**, where the TUI is just one client. `opencode serve --port 4096` exposes `/doc` (the spec, which also generates the SDK), SSE at `/event` and `/global/event`, and session CRUD including `POST /session/:id/fork`, `POST /session/:id/prompt_async` (204, no wait), `summarize`, `revert`, and `POST /session/:id/permissions/:permissionID` with a **`remember?` flag** — always-allow is a protocol field, not a UI affordance. Plugins are in-process TypeScript (`tool.execute.before/after`, `shell.env`, `permission.asked/replied`, `session.*`, `experimental.session.compacting`) and can register new tools. MIT ([server](https://opencode.ai/docs/server/), [plugins](https://opencode.ai/docs/plugins/)).

**可整合的一件东西：** the OpenAPI session/permission surface, including `remember?` on permission replies — copy the shape so every engine answers approvals identically.


### Gemini CLI
The seam is a **declarative, tiered policy engine** — the only terminal here whose permission model is a language, not a mode. Rules are TOML (`~/.gemini/policies/*.toml`; admin tiers at `/etc/gemini-cli/policies`, `/Library/Application Support/GeminiCli/policies`, `C:\ProgramData\gemini-cli\policies`), with `toolName` / `commandPrefix` / argument conditions, decisions `allow | deny | ask_user`, explicit `priority`, and `final_priority = tier_base + (toml_priority / 1000)`. In non-interactive mode `ask_user` degrades to `deny` — a clean headless default. Second seam: git-shadow **checkpointing** (`~/.gemini/history/<hash>` + `~/.gemini/tmp/<hash>/checkpoints`) where `/restore` reverts files *and* conversation *and* re-proposes the tool call ([policy engine](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/policy-engine.md), [checkpointing](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/checkpointing.md)).

**可整合的一件东西：** the TOML rule engine with `allow/deny/ask_user` + tiers + numeric priority, evaluated over *every* engine's tool calls so one policy file governs all slots.

### Aider
The seam is **git as the session store and the undo stack**. Every AI edit is auto-committed with an LLM-written Conventional Commit message; pre-existing dirty changes are committed *first* so user work is never entangled with the model's; `/undo`, `/diff`, `/commit`, `/git` are the history API. Second seam: the **repo map** (tree-sitter symbol graph, ranked, token-budgeted via `--map-tokens`) sent with every request — a genuinely different context strategy from "dump files." Apache-2.0, but **last push 2026-05-22** ([git](https://aider.chat/docs/git.html), [repo map](https://aider.chat/docs/repomap.html)).

**可整合的一件东西：** commit-before-edit + commit-per-change with an LLM commit message, so any slot's work is reviewable and reversible with `git` alone.

### Cursor CLI (and the ACP field)
`agent` is a real proprietary CLI: `-p/--print`, `--force`/`--yolo` (writes are *proposals* without it), `--output-format text|json|stream-json`, `--stream-partial-output`, `--mode agent|plan|ask`, session resume, `CURSOR_API_KEY`, and `agent acp`. The bigger finding is **ACP (Agent Client Protocol, Apache-2.0, Zed)**: JSON-RPC 2.0, newline-delimited, over stdio — `initialize` → `authenticate` → `session/new` | `session/load` → `session/prompt` → `session/update` → `session/cancel`, plus file-system, terminal, permission-request, session-mode/config-option, slash-command, plan, and elicitation methods. Its registry lists **Claude Agent (Zed SDK adapter), Codex CLI (codex-acp adapter), Cursor, Gemini CLI, OpenCode, Goose, GitHub Copilot, Cline, Qwen Code, Kimi CLI, Kiro, Junie, OpenHands, Factory Droid, Mistral Vibe, Augment** and more ([Cursor headless](https://cursor.com/docs/cli/headless), [Cursor ACP](https://cursor.com/docs/cli/acp), [ACP overview](https://agentclientprotocol.com/protocol/v1/overview), [transports](https://agentclientprotocol.com/protocol/v1/transports), [agents](https://agentclientprotocol.com/get-started/agents)).

**可整合的一件东西：** an **ACP client** — one adapter turns ~40 heterogeneous agents into slots with identical prompt/cancel/permission semantics.


---

## Model selection & catalog

| Terminal | Config mechanism | Failover | Local models | Machine-readable catalog? |
|---|---|---|---|---|
| **Claude Code** | `--model <alias\|id>`, `model` setting, `ANTHROPIC_MODEL`; managed `availableModels` allowlist | **Yes**: `--fallback-model a,b` / `fallbackModel[]`; fires on overloaded/unavailable/non-retryable, never on auth/billing/rate-limit/transport; capped at **3** after dedupe; one turn only; also covers subagents | No native Ollama/LM Studio — only via Bedrock/Vertex/Foundry or a gateway base URL | **No** public catalog |
| **Codex** | `config.toml`: `model`, `model_provider`, `model_providers{}` (`base_url`, `env_key`, `wire_api`, `query_params`, `capabilities`, `aws`, `auth`, `gateway_oauth`, **`model_catalog_url`**), `profiles`, `review_model` | `model/rerouted` notification exists; user chains unverified | **Yes, first-class**: `oss_provider = "lmstudio" \| "ollama"` | **Yes** — `model/list` returns typed models (display name, reasoning efforts, modalities, service tiers, upgrade info); context window is separate (`model_context_window`, `model_auto_compact_token_limit`) |
| **OpenCode** | `opencode.json`: `model`, `small_model`, `provider{}` (timeout/chunkTimeout/setCacheKey/apiKey/region), per-agent and per-command `model` | None documented; `small_model` auto-picks cheaper | **Yes** (Ollama / LM Studio / OpenAI-compatible); `disabled_providers` / `enabled_providers` | **Yes via models.dev** — `models.dev/api.json` = **228 providers**, each model with `limit.context`, `limit.output`, `cost`, `reasoning`, `tool_call`, `modalities`, `open_weights` |
| **Gemini CLI** | `--model/-m`, `settings.json`, `model-routing` config, `auto` | `model-routing.md` exists; chain semantics unverified | Gemma via `core/gemma-setup`, `local-model-routing` doc | None found |
| **Aider** | `.aider.conf.yml`, CLI, LiteLLM strings; `--weak-model`, `--editor-model`, `--architect` | **No** documented failover | **Yes** via LiteLLM: Ollama, LM Studio, OpenAI-compatible | **Yes (bundled)**: `model-settings.yml` (edit_format, weak_model_name, use_repo_map, lazy) and `model-metadata.json` (**`max_input_tokens`/`max_output_tokens`/costs**) — but it is JSONC, needs comment-stripping |
| **Cursor CLI** | `--model`, dashboard, `CURSOR_API_KEY` | Not documented | Not documented | Unverified (proprietary) |

**Takeaway:** only **Codex** and **models.dev** give a *typed, fetchable* catalog; only Aider and Codex declare context windows in-repo. Read models.dev fleet-wide, `model/list` + `model_context_window` for Codex truth.


---

## Session persist / resume / fork

| Terminal | Format | Path | Documented? | Programmatic? |
|---|---|---|---|---|
| **Claude Code** | **JSONL**, one object per message/tool-use/metadata line | `~/.claude/projects/<cwd-with-non-alnum-replaced-by-`-`>/<session-id>.jsonl`; >200 chars → truncate + path hash; overridable via `CLAUDE_CONFIG_DIR` + `CLAUDE_CODE_PROJECT_DIR_NAME` | Path **documented**; **line format explicitly declared internal and version-unstable** ("scripts that parse these files directly can break on any release") | Yes — `--resume <id\|name\|transcript-path>`, `--continue`, `--fork-session`, `--session-id <uuid>`, `/branch`; SDK is the sanctioned path |
| **Codex** | **JSONL** rollouts | `~/.codex/sessions/rollout-<ISO8601>-<uuid>.jsonl` (repo docstring shows the `jq -C .` idiom) | Yes (source doc); schema is generated JSON Schema | **Strongest**: `thread/list`, `thread/read`, `thread/turns/list`, `thread/items/list`, `thread/resume`, `thread/fork`, `thread/archive`, `thread/compact/start`, `thread/revert`, `thread/inject_items` |
| **OpenCode** | v1: per-project JSON blobs; **v2 has moved to SQLite** (drizzle schema + timestamped migrations, incl. `normalize_storage_paths`) | Docs: `~/.local/share/opencode/` with `auth.json`, `log/`, `project/<slug>/storage/` (or `global/storage/` for non-git). v2 DB path is source-verified, doc-undescribed | Partially | **Yes**: `POST /session` , `POST /session/:id/fork {messageID?}`, `POST /session/:id/revert\|unrevert`, `GET /session/:id/message`; realtime via SSE `/event` |
| **Gemini CLI** | JSON chats + shadow-git checkpoints | `~/.gemini/tmp/<project_hash>/chats/`; checkpoints at `~/.gemini/tmp/<project_hash>/checkpoints` + git at `~/.gemini/history/<project_hash>` | Yes | Partial: `--resume latest\|N\|uuid`, `--list-sessions`, `/resume`, `/restore`; no documented fork |
| **Aider** | Markdown | `.aider.chat.history.md`, `.aider.input.history` | Yes (informally) | `--message`, Python `Coder` API; **no protocol, no fork** |
| **Cursor CLI** | proprietary | not documented | No | `--resume` exists; fork **unverified** |

---

## Driving protocols compared

| Surface | Shape | Transport | Host tools? | Approvals in-band? | Fork/resume in-band? | Stability |
|---|---|---|---|---|---|---|
| `claude -p --output-format stream-json` (+ `--input-format stream-json`) | NDJSON duplex, Claude-specific | stdio | via MCP / `--permission-prompt-tool` | yes | `--resume`, `--fork-session`, `--session-id` | undocumented event schema |
| **Codex `app-server`** | **JSON-RPC 2.0**, published JSON Schema | `stdio://`, `unix://[PATH]`, `ws://IP:PORT` | **native** (`item/tool/call`) | yes, typed | `thread/resume`, `thread/fork` | schema-checked artifacts |
| **OpenCode server** | REST + SSE, **OpenAPI 3.1** at `/doc` | HTTP(+SSE) | no (plugins are in-process TS) | yes (`permissions/:id` + `remember?`) | `POST /session/:id/fork` | versioned spec |
| **ACP** | **JSON-RPC 2.0**, newline-delimited | stdio (streamable HTTP = draft) | via MCP handshake in `initialize` | yes (`session/request_permission`) | `session/load` (fork unverified) | spec + v2 draft; ~40-agent registry |
| MCP | JSON-RPC tool/resource protocol | stdio / HTTP / SSE | — (this *is* tool exposure) | elicitation only | n/a | mature |
| Aider / Cursor PTY | text | PTY | no | terminal prompt only | no | n/a |


**Recommendation — layered, not singular.**

1. **ACP is the universal driving protocol.** It is the only surface with (a) a real spec, (b) an *agent registry* already containing Claude Agent, Codex, Cursor, Gemini CLI, OpenCode, Goose, Copilot, Qwen, Kimi, Kiro, OpenHands and Droid, and (c) the semantics a console needs — `session/new`, `session/prompt`, `session/cancel`, `session/request_permission`, `session/update`, plans, modes, config options, terminals. Build **one Rust ACP client** and most of the ecosystem becomes slots for free, with PTY fallback for anything unregistered.
2. **Use native protocols behind an ACP-shaped facade where they are strictly richer.** Codex `app-server` adds typed approval requests, `item/tool/call`, `model/list`, `permissionProfile/list`, `hooks/list` and four transports. OpenCode's OpenAPI server is the best HTTP-native option and is trivial to consume from Rust.
3. **Normalize internally to one event enum** (`ItemStarted | ItemDelta | ToolCall | ApprovalRequested | TurnCompleted | …`) modelled on Codex's `Item`/`Turn`/`Thread` vocabulary.


---

## Hook event comparison

**Claude Code — 33 events as of 2026-10-14** (the brief's "30" is stale):
`SessionStart`, `Setup`, `InstructionsLoaded`, `UserPromptSubmit`, `UserPromptExpansion`, `MessageDisplay`, `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch`, `PermissionDenied`, `Notification`, `SubagentStart`, `SubagentStop`, `TaskCreated`, `TaskCompleted`, `Stop`, `StopFailure`, `TeammateIdle`, `ConfigChange`, `CwdChanged`, `DirectoryAdded`, `FileChanged`, `WorktreeCreate`, `WorktreeRemove`, `PreCompact`, `PostCompact`, `PreModelSwitch`, `PostModelSwitch`, `SessionEnd`, `Elicitation`, `ElicitationResult`. Handlers may be shell, **HTTP endpoint, MCP tool call, LLM prompt, or subagent**; hooks can run async/background ([hooks reference](https://code.claude.com/docs/en/hooks)).

| Concept | Claude Code | Codex (12) | Gemini CLI (11) | OpenCode | DSH (7) |
|---|---|---|---|---|---|
| Session start/end | `SessionStart`, `Setup` / `SessionEnd` | `sessionStart` / `sessionEnd` | `SessionStart` / `SessionEnd` | `session.created` / `session.deleted` | ✅ |
| User prompt | `UserPromptSubmit`, `UserPromptExpansion` | `userPromptSubmit` | — | — | ✅ |
| Before tool | `PreToolUse` | `preToolUse` | `BeforeTool` | `tool.execute.before` | ✅ |
| After tool | `PostToolUse`, `PostToolUseFailure`, `PostToolBatch` | `postToolUse` | `AfterTool` | `tool.execute.after` | ✅ |
| Permission asked/replied | `PermissionRequest`, `PermissionDenied` | `permissionRequest` | (policy engine instead) | `permission.asked`, `permission.replied` | ❌ |
| Stop / turn end | `Stop`, `StopFailure` | `stop`, `interrupt` | `AfterAgent` | `session.idle` | ✅ |
| Subagent start/stop | `SubagentStart`, `SubagentStop` | `subagentStart`, `subagentStop` | — | — | ✅ |
| Compaction | `PreCompact`, `PostCompact` | `preCompact`, `postCompact` | `PreCompress` | `experimental.session.compacting` | ❌ |
| Model call | `PreModelSwitch`, `PostModelSwitch` | — | `BeforeModel`, `AfterModel`, `BeforeToolSelection` | — | ❌ |
| Agent turn | — | — | `BeforeAgent`, `AfterAgent` | — | ❌ |
| Files / worktree / fs | `FileChanged`, `CwdChanged`, `DirectoryAdded`, `WorktreeCreate`, `WorktreeRemove` | — | — | `file.edited`, `file.watcher.updated` | ❌ |
| Notification / tasks / config / elicitation | `Notification`, `MessageDisplay`, `TaskCreated`, `TaskCompleted`, `ConfigChange`, `Elicitation`, `ElicitationResult`, `InstructionsLoaded`, `TeammateIdle` | `mcpServer/elicitation/request`, `configWarning` | `Notification` | `tui.toast.show` | ❌ |

**Handler compatibility:** Codex already mirrors Claude Code's vocabulary almost exactly (12 events, camelCase) with the same handler kinds (`command | mcpTool | prompt | agent`). **Cheapest bridge upgrade:** adopt Codex's 12-event list verbatim as canonical naming, then add four — `PermissionRequest`, `PreCompact`/`PostCompact`, `FileChanged`, `BeforeModel`/`AfterModel`. `PermissionRequest` is the one that changes product behaviour: it lets the console veto a tool call *before* the engine's own prompt appears.


---

## Skills convergence

Convergence is real and now spec-backed (**Agent Skills**, agentskills.io). The least-common-denominator format is:

```
<skills-root>/<skill-name>/SKILL.md          # YAML frontmatter + markdown body
<skills-root>/<skill-name>/references/*.md   # optional, loaded on demand
<skills-root>/<skill-name>/scripts/*         # optional executables
```

**Frontmatter LCD** — `name` (required, `^[a-z0-9]+(-[a-z0-9]+)*$`, 1–64 chars, must equal the directory name, no `--`) and `description` (required, 1–1024 chars). Everything else is optional and non-portable: `license`, `compatibility`, `metadata` (string→string map) are in the Agent Skills spec; `allowed-tools` is Claude-specific; OpenCode says explicitly *"put OpenCode-specific behavior in `metadata` instead of adding custom top-level fields"* — `metadata["opencode/slash"]`, `metadata["opencode/autoinvoke"]`.

**Directory discovery — the actual convergence:**

| Root | Claude Code | Codex | Gemini CLI | OpenCode |
|---|---|---|---|---|
| `~/.claude/skills/`, `.claude/skills/` | ✅ | — | — | ✅ |
| `~/.agents/skills/`, `.agents/skills/` | ❌ | ✅ | ✅ (**beats `.gemini/skills` in-tier**) | ✅ |
| `~/.gemini/skills/`, `.gemini/skills/` | — | — | ✅ | — |
| `~/.config/opencode/skills/`, `.opencode/skills/` | — | — | — | ✅ |
| plugin/extension-bundled `skills/` | ✅ | ✅ (repo samples carry `SKILL.md` + `references/` + `scripts/` + `agents/openai.yaml` + `assets/`) | ✅ | ✅ |

**LCD for a console:** author skills at **`.agents/skills/<name>/SKILL.md`** with only `name` + `description`, and put engine-specific data under `metadata`. Codex, Gemini CLI and OpenCode read that path; **Claude Code does not** — so the console must symlink or mirror into `.claude/skills/` and `~/.gemini/skills/`. Progressive disclosure (name+description in the prompt, body loaded on activation) is consistent across all four; OpenCode's loader even tags the expansion with *"Base directory for this skill … relative paths (e.g. `scripts/`, `reference/`) are relative to this base directory."*


---

## Approval UX comparison

| Terminal | What is gated | Granularity | "Always allow" memory | Narrow gate (build/test free, shell asks) |
|---|---|---|---|---|
| **Claude Code** | Bash, edits, WebFetch domains, MCP tools; protected/critical paths always guarded | **Per-tool and per-path/command-prefix** (`Bash(git log *)`, per-path Read rules) | **Yes, two-tier**: "don't ask again" writes a scoped rule to `.claude/settings.local.json` at repo root (survives sessions); file-edit approvals last only the session; compound commands save up to 5 sub-rules | **Closest analogue**: `--allowedTools` pre-approves named tools (`Read`, `Bash(npm test)`) leaving exec gated; auto mode adds a server-side risk classifier |
| **Codex** | Command exec, file changes, permissions, MCP elicitation, sandbox escapes/network | **Typed per-request**; `AskForApproval` = `untrusted \| on-request \| never` plus a `granular` per-category object | Not a UI memory — bytes live in config + execpolicy `.rules` (Starlark) and `permissionProfile/list` | **Yes**: sandbox modes `read-only \| workspace-write \| danger-full-access` mean in-workspace build/test runs unattended while escapes ask. `ApprovalsReviewer: auto_review` = an LLM subagent risk-reviews instead of a human |
| **OpenCode** | `edit`, `bash`, `webfetch`, `skill` | **Per-tool × wildcard pattern × per-agent override**: `{"bash": {"git *": "allow", "*": "ask"}}` | **Yes, explicit**: the permission reply body carries `remember?` over HTTP | **Yes** — deny-by-default `*` plus an allow-list of build/test patterns gives exactly the Xcode-27 shape |
| **Gemini CLI** | Any tool, via policy rules | **Richest**: conditions on `toolName`, `commandPrefix`, args, approval mode; `allow \| deny \| ask_user`; numeric priority; Default/User/Admin tiers | Not a UI memory — persisted as rules in `~/.gemini/policies/*.toml` | **Yes**: `run_shell_command` + `commandPrefix` rules let `pytest`/`npm test` be `allow` while `git push` is `ask_user`; headless `ask_user` → `deny` |
| **Aider** | Every LLM-suggested edit | All-or-nothing per edit; `--yes` bypasses | No | **No** — nothing distinguishes build/test from shell |
| **Cursor CLI** | File changes only in print mode | `--force`/`--yolo` = all-or-nothing | Unverified | **No** |

**Who has what:** real persistent "always allow" — **Claude Code** (scoped JSON rules at repo root) and **OpenCode** (`remember?` protocol field). **Narrow-but-real gates** — **Gemini CLI** (policy engine), **Codex** (sandbox modes + granular ask), **OpenCode** (wildcard allow-lists), **Claude Code** (`--allowedTools` + auto-mode classifier). Build the console's gate by composing: OpenCode's reply shape (`{response, remember?}`), Gemini's TOML rule file, Codex's typed categories, Claude Code's repo-root persistence rule.


---

## Top 10 mechanisms worth copying

1. **Codex `app-server` as the slot protocol** (Codex) — one JSON-RPC client over stdio/ws turns slots into typed `Thread`/`Turn`/`Item` streams instead of PTY buffers, with schemas to codegen from.
2. **ACP client adapter** (ACP/Zed) — one Rust client unlocks ~40 engines (Claude, Codex, Cursor, Gemini, OpenCode, Goose, Copilot…); each slot becomes `acp` | `native` | `pty`.
3. **`item/tool/call` host-owned tools** (Codex) — expose console capabilities (scheduler, cost meter, cross-slot messaging) as tools any engine can call, with no MCP server.
4. **In-band permission reply with `remember?`** (OpenCode) — one uniform approval RPC for every engine, making "always allow" a console feature rather than a keystroke scrape.
5. **Declarative policy engine with tiers + priority** (Gemini CLI) — one TOML rule set for all slots yields the Xcode-27 gate: `allow` build/test prefixes, `ask_user` shell, admin-tier overrides.
6. **Repo-root-persistent scoped allow rules** (Claude Code) — save "don't ask again" as `Bash(npm test *)` patterns in a repo-local file, reviewable in git and shared across slots on one checkout.
7. **LLM-reviewer approval routing** (Codex `auto_review`) — a cheap subagent triages approvals; add a confidence threshold so only real escalations reach the human.
8. **NDJSON duplex turn engine** (Claude Code `--input-format/--output-format stream-json`) — the fallback for engines without a protocol: long-lived process, push turns in, stream events out, `--resume`/`--fork-session` for slot identity.
9. **Shadow-git checkpoints bundling files + conversation + pending tool call** (Gemini CLI) — `/restore` gives a per-slot "rewind to before that tool call" a JSONL-only store cannot express.
10. **Commit-before-edit and commit-per-change with LLM commit messages** (Aider) — every slot leaves the worktree clean and attributable, making a multi-slot console safe against interleaved edits.

*Runners-up:* Codex's `permissionProfile/list` + `hooks/list` introspection (render the host's real capability surface instead of guessing); Aider's ranked tree-sitter repo map as a context-budget strategy.


---

## Unverified / could not confirm

- **Claude Code event count "30"** in the brief is stale — the docs list **33** events as of 2026-10-14, so DSH's bridge is 7/33, not 7/30.
- **Claude Code JSONL line schema** — path and JSONL-ness are documented and confirmed; the *per-line object schema* is explicitly internal and version-unstable, so any parser is unsupported. `/export`, the SDK, or hook `transcript_path` are the sanctioned reads.
- **DSH's 7-event bridge** — the names were supplied by the delegating agent; I verified the Claude Code side of the mapping, not DSH's implementation.
- **Codex hosted docs unreachable** — `developers.openai.com/codex/*` returned HTTP 403 to both curl and the fetch tool. All Codex claims come from Apache-2.0 repo source, generated JSON Schema, and the OpenAI engineering post.
- **Codex fallback chains** — `model/rerouted` exists and `model_catalog_url` is configurable, but no user-configured fallback *chain* equivalent to Claude Code's `fallbackModel` was found.
- **OpenCode v2 storage** — the *docs* say `~/.local/share/opencode/project/<slug>/storage/` JSON; the *source* on `dev` has a SQLite + drizzle layer with migrations. Version-dependent; no doc statement about the v2 database exists. **OpenCode failover** also unverified — no fallback/retry-chain key found.
- **Gemini CLI model fallback** — `model-routing.md` exists but its failover triggers were not read in full. Its extension install directory (conventionally `~/.gemini/extensions/`) was also not located, though `gemini-extension.json` and `gemini extensions install <source>` are confirmed.
- **Aider** — no failover documented, no machine-readable output found (PTY/plain text only); repo unpublished since 2026-05-22 but formal unmaintained status unverified. `model-metadata.json` is JSONC (`//` comments) so it fails a strict parse.
- **Cursor CLI** — sessions/resume, hooks, permission memory, and any model catalog unverified; docs are client-rendered and only headless/ACP/overview content extracted. Proprietary, so no source fallback.
- **ACP `session/fork`** — v1 documents `session/new` and `session/load` only; fork-equivalent behaviour via `load` is unverified. A **v2 draft** with different method names exists and was not read.
- **Claude Code + `.agents/skills`** — its skills doc lists only `~/.claude/skills` and `.claude/skills` (+ managed, nested, plugin, `--add-dir`); a search for `.agents` on that page returned zero hits. Assume unsupported.


