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
  | { kind: "token_usage"; [k: string]: unknown }
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
  async revealAuditLog() {
    return invoke<string>("reveal_audit_log");
  },
  async revealConfigDir() {
    return invoke<string>("reveal_config_dir");
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
