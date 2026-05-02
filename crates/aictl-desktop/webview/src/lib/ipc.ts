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
  async sendMessage(text: string, autoAccept: boolean, sessionId?: string) {
    return invoke<void>("send_message", {
      args: {
        text,
        session_id: sessionId ?? null,
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

  // -- sessions ----
  async listSessions() {
    return invoke<SessionRow[]>("list_sessions");
  },
  async loadSession(id: string) {
    return invoke<string>("load_session", { id });
  },
  async deleteSession(id: string) {
    return invoke<void>("delete_session", { id });
  },
  async newIncognitoSession() {
    return invoke<void>("new_incognito_session");
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
  async openUrl(url: string) {
    return invoke<void>("open_url", { url });
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
