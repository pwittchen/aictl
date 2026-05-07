// Typed wrappers around Tauri's `invoke` / `listen`. Centralizing the
// command names here keeps frontend code free of magic strings — every
// handler in `crates/aictl-desktop/src/commands/` has exactly one
// matching wrapper below.

import { invoke } from "@tauri-apps/api/core";
import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";

export type AgentEvent =
  | { kind: "spinner_start"; message: string }
  | { kind: "spinner_stop" }
  | { kind: "reasoning"; text: string }
  | { kind: "stream_begin" }
  | { kind: "stream_chunk"; text: string }
  | { kind: "stream_suspend" }
  | { kind: "stream_end" }
  | { kind: "tool_auto"; tool: string; input: string }
  | {
      kind: "tool_approval_request";
      id: number;
      tool: string;
      input: string;
    }
  | { kind: "tool_result"; text: string }
  | { kind: "answer"; text: string }
  | { kind: "error"; text: string }
  | { kind: "warning"; text: string }
  | {
      kind: "token_usage";
      model: string;
      final_answer: boolean;
      input_tokens: number;
      output_tokens: number;
      cache_creation_input_tokens: number;
      cache_read_input_tokens: number;
      tool_calls: number;
      elapsed_ms: number;
      context_pct: number;
      memory: string;
    }
  | { kind: "summary"; [k: string]: unknown }
  | { kind: "progress_begin"; id: number; label: string; total: number | null }
  | {
      kind: "progress_update";
      id: number;
      current: number;
      message: string | null;
    }
  | { kind: "progress_end"; id: number; message: string | null };

export interface WorkspaceState {
  path: string | null;
  stale: boolean;
  error: string | null;
}

export interface SessionRow {
  id: string;
  name: string | null;
  size: number;
  modified_secs: number;
  active: boolean;
}

export interface LoadedMessage {
  kind: "system" | "user" | "assistant" | "tool_result";
  text: string;
}

export interface LoadSessionResult {
  id: string;
  name: string | null;
  messages: LoadedMessage[];
}

export interface ActiveSession {
  id: string | null;
  name: string | null;
  incognito: boolean;
}

export interface TranscriptMessage {
  kind: "system" | "user" | "assistant" | "tool_result";
  text: string;
}

export interface TranscriptUpdate {
  messages: TranscriptMessage[];
  prompt: string | null;
  popped: number;
}

export type ToolDecision = "allow" | "deny" | "auto_accept";

export interface ModelEntry {
  provider: string;
  model: string;
}

export interface ActiveModel {
  provider: string | null;
  model: string | null;
}

export interface ConfigEntry {
  key: string;
  value: string | null;
}

export interface KeyRow {
  name: string;
  label: string;
  location: "unset" | "plain" | "keyring" | "both";
}

export interface KeyBackend {
  available: boolean;
  name: string;
}

export interface KeysBulkResult {
  migrated: number;
  already: number;
  skipped: number;
  errors: [string, string][];
}

export interface ToolRow {
  name: string;
  description: string;
  disabled: boolean;
}

export interface McpServerRow {
  name: string;
  transport: string;
  command: string;
  args: string[];
  url: string;
  enabled: boolean;
  state: string;
  state_detail: string | null;
  tool_count: number;
}

export interface McpStatus {
  enabled: boolean;
  config_path: string;
  config_exists: boolean;
  servers: McpServerRow[];
}

export interface HookRow {
  idx: number;
  event: string;
  matcher: string;
  command: string;
  timeout_secs: number;
  enabled: boolean;
}

export interface HooksStatus {
  config_path: string | null;
  hooks: HookRow[];
}

export interface SkillRow {
  name: string;
  description: string;
  source: string | null;
  category: string | null;
  origin: string;
  official: boolean;
  dir: string;
}

export interface AgentRow {
  name: string;
  description: string | null;
  source: string | null;
  category: string | null;
  origin: string;
  official: boolean;
  path: string;
}

/// Remote-catalogue listing — same shape for skills and agents. `state`
/// is the upstream-vs-local relation: "not_pulled" means the user
/// hasn't installed it yet (a Pull button shows up); "up_to_date" and
/// "upstream_newer" surface only when the catalogue tab refreshes after
/// an installed entry exists.
export interface RemoteCatalogueRow {
  name: string;
  description: string | null;
  category: string | null;
  state: "not_pulled" | "up_to_date" | "upstream_newer";
}

export type PullOutcome = "installed" | "overwritten" | "skipped";

export interface SkillView {
  name: string;
  description: string;
  origin: string;
  path: string;
  raw: string;
  body: string;
}

export interface AgentView {
  name: string;
  description: string | null;
  origin: string;
  path: string;
  raw: string;
  body: string;
}

export interface PluginRow {
  name: string;
  description: string;
  entrypoint: string;
  requires_confirmation: boolean;
  timeout_secs: number | null;
}

export interface PluginsStatus {
  enabled: boolean;
  plugins_dir: string;
  plugins: PluginRow[];
}

export interface StatsBucket {
  label: string;
  sessions: number;
  requests: number;
  llm_calls: number;
  tool_calls: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
  models: { model: string; count: number }[];
}

export interface StatsSnapshot {
  day_count: number;
  today: StatsBucket;
  month: StatsBucket;
  overall: StatsBucket;
}

export interface DailyPoint {
  date: string;
  requests: number;
  llm_calls: number;
  tool_calls: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
}

export interface ServerStatus {
  host: string | null;
  master_key_set: boolean;
  fully_configured: boolean;
}

export interface ServerProbeResult {
  healthz_ok: boolean;
  healthz_status: number | null;
  healthz_error: string | null;
  models_ok: boolean;
  models_status: number | null;
  models_error: string | null;
  model_count: number | null;
}

export interface OllamaStatus {
  host: string;
  default_host: string;
  overridden: boolean;
}

export interface OllamaProbeResult {
  ok: boolean;
  status: number | null;
  error: string | null;
  model_count: number | null;
  sample_models: string[];
}

export interface CatalogEntryRow {
  label: string;
  spec: string;
  size_label: string;
}

export interface LocalModelRow {
  name: string;
  size_bytes: number;
}

export interface GgufStatus {
  inference_available: boolean;
  dir: string;
  models: LocalModelRow[];
  catalog: CatalogEntryRow[];
}

export interface MlxStatus {
  inference_available: boolean;
  host_supports_mlx: boolean;
  dir: string;
  models: LocalModelRow[];
  catalog: CatalogEntryRow[];
}

export interface LocalModelsStatus {
  gguf: GgufStatus;
  mlx: MlxStatus;
}

export interface ContextStatus {
  model: string | null;
  provider: string | null;
  last_input_tokens: number;
  last_output_tokens: number;
  context_limit: number;
  messages: number;
  max_messages: number;
  token_pct: number;
  message_pct: number;
  context_pct: number;
  auto_compact_threshold: number;
  auto_compact_overridden: boolean;
}

export const ipc = {
  // -- workspace ----
  async getWorkspace() {
    return invoke<WorkspaceState>("get_workspace");
  },
  async setWorkspace(path: string) {
    return invoke<WorkspaceState>("set_workspace", { path });
  },
  async pickWorkspace() {
    return invoke<string | null>("pick_workspace");
  },
  async useDefaultWorkspace() {
    return invoke<WorkspaceState>("use_default_workspace");
  },
  async defaultWorkspacePath() {
    return invoke<string>("default_workspace_path");
  },

  // -- chat ----
  async sendMessage(text: string, autoAccept: boolean) {
    return invoke<void>("send_message", {
      args: {
        text,
        auto_accept: autoAccept,
      },
    });
  },
  async stopTurn() {
    return invoke<void>("stop_turn");
  },
  async toolApprovalResponse(id: number, decision: ToolDecision) {
    return invoke<void>("tool_approval_response", {
      args: { id, decision },
    });
  },
  async clearChat() {
    return invoke<TranscriptUpdate>("clear_chat");
  },
  async retryLast() {
    return invoke<TranscriptUpdate>("retry_last");
  },
  async undoLast(n = 1) {
    return invoke<TranscriptUpdate>("undo_last", { args: { n } });
  },
  async compactChat() {
    return invoke<TranscriptUpdate>("compact_chat");
  },

  // -- sessions ----
  async listSessions() {
    return invoke<SessionRow[]>("list_sessions");
  },
  async loadSession(id: string) {
    return invoke<LoadSessionResult>("load_session", { id });
  },
  async deleteSession(id: string) {
    return invoke<void>("delete_session", { id });
  },
  async clearSessions() {
    return invoke<void>("clear_sessions");
  },
  async renameSession(id: string, name: string) {
    return invoke<void>("rename_session", { args: { id, name } });
  },
  async newSession() {
    return invoke<void>("new_session");
  },
  async newIncognitoSession() {
    return invoke<void>("new_incognito_session");
  },
  async getActiveSession() {
    return invoke<ActiveSession>("get_active_session");
  },

  // -- models ----
  async listModels() {
    return invoke<ModelEntry[]>("list_models");
  },
  async getActiveModel() {
    return invoke<ActiveModel>("get_active_model");
  },
  async setActiveModel(provider: string, model: string) {
    return invoke<ActiveModel>("set_active_model", { provider, model });
  },

  // -- system ----
  async version() {
    return invoke<string>("version");
  },
  async buildProfile() {
    return invoke<"debug" | "release">("build_profile");
  },
  async buildTime() {
    return invoke<string>("build_time");
  },
  async buildCommit() {
    return invoke<string>("build_commit");
  },
  async readWorkspaceImage(path: string) {
    return invoke<{ base64: string; media_type: string }>(
      "read_workspace_image",
      { path },
    );
  },
  async revealAuditLog() {
    return invoke<void>("reveal_audit_log");
  },
  async revealConfigDir() {
    return invoke<void>("reveal_config_dir");
  },
  async openUrl(url: string) {
    return invoke<void>("open_url", { url });
  },

  // -- settings ----
  async configDump() {
    return invoke<ConfigEntry[]>("config_dump");
  },
  async configValue(key: string) {
    return invoke<string | null>("config_value", { args: { key } });
  },
  async configWrite(key: string, value: string) {
    return invoke<void>("config_write", { args: { key, value } });
  },
  async configClear(key: string) {
    return invoke<boolean>("config_clear", { args: { key } });
  },
  async keysStatus() {
    return invoke<KeyRow[]>("keys_status");
  },
  async keysBackend() {
    return invoke<KeyBackend>("keys_backend");
  },
  async keysSet(name: string, value: string) {
    return invoke<string>("keys_set", { args: { name, value } });
  },
  async keysClear(name: string) {
    return invoke<string>("keys_clear", { args: { name } });
  },
  async keysLock(name: string) {
    return invoke<string>("keys_lock", { args: { name } });
  },
  async keysUnlock(name: string) {
    return invoke<string>("keys_unlock", { args: { name } });
  },
  async keysLockAll() {
    return invoke<KeysBulkResult>("keys_lock_all");
  },
  async keysUnlockAll() {
    return invoke<KeysBulkResult>("keys_unlock_all");
  },
  async toolsList() {
    return invoke<ToolRow[]>("tools_list");
  },
  async toolSetDisabled(name: string, disabled: boolean) {
    return invoke<boolean>("tool_set_disabled", {
      args: { name, disabled },
    });
  },

  // -- mcp ----
  async mcpStatus() {
    return invoke<McpStatus>("mcp_status");
  },
  async mcpToggle(name: string, enabled: boolean) {
    return invoke<boolean>("mcp_toggle", { args: { name, enabled } });
  },
  async mcpCreate(payload: {
    name: string;
    transport?: "stdio" | "http" | "sse";
    command?: string;
    args?: string[];
    env?: Record<string, string>;
    url?: string;
    headers?: Record<string, string>;
    timeoutSecs?: number;
    overwrite: boolean;
  }) {
    return invoke<void>("mcp_create", {
      args: {
        name: payload.name,
        transport: payload.transport ?? "stdio",
        command: payload.command ?? "",
        args: payload.args ?? [],
        env: payload.env ?? {},
        url: payload.url ?? "",
        headers: payload.headers ?? {},
        timeout_secs: payload.timeoutSecs ?? null,
        overwrite: payload.overwrite,
      },
    });
  },

  // -- hooks ----
  async hooksStatus() {
    return invoke<HooksStatus>("hooks_status");
  },
  async hookToggle(event: string, idx: number, enabled?: boolean) {
    return invoke<boolean>("hook_toggle", {
      args: { event, idx, enabled: enabled ?? null },
    });
  },
  async hookDelete(event: string, idx: number) {
    return invoke<void>("hook_delete", { args: { event, idx } });
  },
  async hookCreate(
    event: string,
    matcher: string,
    command: string,
    timeoutSecs?: number,
  ) {
    return invoke<void>("hook_create", {
      args: {
        event,
        matcher,
        command,
        timeout_secs: timeoutSecs ?? null,
      },
    });
  },

  // -- skills ----
  async skillsList() {
    return invoke<SkillRow[]>("skills_list");
  },
  async skillDelete(name: string, origin: string) {
    return invoke<void>("skill_delete", { args: { name, origin } });
  },
  async skillView(name: string, origin: string) {
    return invoke<SkillView>("skill_view", { args: { name, origin } });
  },
  async skillLoad(name: string) {
    return invoke<void>("skill_load", { args: { name } });
  },
  async skillUnload() {
    return invoke<void>("skill_unload");
  },
  async skillLoaded() {
    return invoke<string | null>("skill_loaded");
  },
  async skillsListRemote() {
    return invoke<RemoteCatalogueRow[]>("skills_list_remote");
  },
  async skillPull(name: string, overwrite: boolean) {
    return invoke<PullOutcome>("skill_pull", { args: { name, overwrite } });
  },
  async skillSave(
    name: string,
    description: string,
    body: string,
    overwrite: boolean,
  ) {
    return invoke<"installed" | "overwritten">("skill_save", {
      args: { name, description, body, overwrite },
    });
  },
  async skillGenerate(name: string, description: string) {
    return invoke<string>("skill_generate", {
      args: { name, description },
    });
  },

  // -- agents ----
  async agentsList() {
    return invoke<AgentRow[]>("agents_list");
  },
  async agentDelete(name: string, origin: string) {
    return invoke<void>("agent_delete", { args: { name, origin } });
  },
  async agentView(name: string, origin: string) {
    return invoke<AgentView>("agent_view", { args: { name, origin } });
  },
  async agentLoad(name: string) {
    return invoke<void>("agent_load", { args: { name } });
  },
  async agentUnload() {
    return invoke<void>("agent_unload");
  },
  async agentLoaded() {
    return invoke<string | null>("agent_loaded");
  },
  async agentsListRemote() {
    return invoke<RemoteCatalogueRow[]>("agents_list_remote");
  },
  async agentPull(name: string, overwrite: boolean) {
    return invoke<PullOutcome>("agent_pull", { args: { name, overwrite } });
  },
  async agentSave(name: string, body: string, overwrite: boolean) {
    return invoke<"installed" | "overwritten">("agent_save", {
      args: { name, body, overwrite },
    });
  },
  async agentGenerate(name: string, description: string) {
    return invoke<string>("agent_generate", {
      args: { name, description },
    });
  },

  // -- plugins ----
  async pluginsStatus() {
    return invoke<PluginsStatus>("plugins_status");
  },
  async pluginSave(args: {
    name: string;
    description: string;
    body: string;
    requiresConfirmation: boolean;
    timeoutSecs?: number;
    overwrite: boolean;
  }) {
    return invoke<"installed" | "overwritten">("plugin_save", {
      args: {
        name: args.name,
        description: args.description,
        body: args.body,
        requires_confirmation: args.requiresConfirmation,
        timeout_secs: args.timeoutSecs ?? null,
        overwrite: args.overwrite,
      },
    });
  },
  async pluginDelete(name: string) {
    return invoke<void>("plugin_delete", { args: { name } });
  },

  // -- stats ----
  async statsSnapshot() {
    return invoke<StatsSnapshot>("stats_snapshot");
  },
  async statsClear() {
    return invoke<void>("stats_clear");
  },
  async statsDaily(days: number) {
    return invoke<DailyPoint[]>("stats_daily", { days });
  },

  // -- server ----
  async serverStatus() {
    return invoke<ServerStatus>("server_status");
  },
  async serverProbe() {
    return invoke<ServerProbeResult>("server_probe");
  },
  async ollamaStatus() {
    return invoke<OllamaStatus>("ollama_status");
  },
  async ollamaProbe() {
    return invoke<OllamaProbeResult>("ollama_probe");
  },

  // -- context ----
  async contextStatus() {
    return invoke<ContextStatus>("context_status");
  },

  // -- local models (gguf / mlx) ----
  async localModelsStatus() {
    return invoke<LocalModelsStatus>("local_models_status");
  },
  async localModelsPullGguf(spec: string, name?: string) {
    return invoke<{ label: string }>("local_models_pull_gguf", {
      args: { spec, name: name ?? null },
    });
  },
  async localModelsPullMlx(spec: string, name?: string) {
    return invoke<{ label: string }>("local_models_pull_mlx", {
      args: { spec, name: name ?? null },
    });
  },
  async localModelsRemoveGguf(name: string) {
    return invoke<void>("local_models_remove_gguf", { name });
  },
  async localModelsRemoveMlx(name: string) {
    return invoke<void>("local_models_remove_mlx", { name });
  },
  async localModelsClearGguf() {
    return invoke<number>("local_models_clear_gguf");
  },
  async localModelsClearMlx() {
    return invoke<number>("local_models_clear_mlx");
  },

  // -- events ----
  onAgentEvent(cb: (e: AgentEvent) => void): Promise<UnlistenFn> {
    return listen<AgentEvent>("agent_event", (evt: Event<AgentEvent>) =>
      cb(evt.payload),
    );
  },
  onWorkspaceChanged(
    cb: (path: string | null) => void,
  ): Promise<UnlistenFn> {
    return listen<{ path: string | null }>("workspace_changed", (e) =>
      cb(e.payload.path),
    );
  },
};
