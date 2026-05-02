import type { Component } from "solid-js";
import { Show } from "solid-js";

import type { WorkspaceState } from "../lib/ipc";

interface Props {
  workspace: WorkspaceState;
  onPickWorkspace: () => void;
  turnInFlight: boolean;
  onStop: () => void;
  sidebarVisible: boolean;
  onToggleSidebar: () => void;
  /// Most recent context-window usage as a 0..100 percentage. `null`
  /// before the first turn finishes — the meter is hidden in that
  /// state to avoid showing a 0% bar that doesn't reflect anything
  /// real.
  contextPct: number | null;
  contextTokens: { input: number; limit: number } | null;
}

const Titlebar: Component<Props> = (props) => {
  const label = () => {
    const path = props.workspace.path;
    if (!path) return "No workspace";
    const home = path.replace(/^\/Users\/[^/]+/, "~");
    if (home.length <= 36) return home;
    return `…${home.slice(-34)}`;
  };

  return (
    <header class="titlebar">
      <div class="titlebar-drag" data-tauri-drag-region />
      <div class="titlebar-content">
        <span class="brand">aictl</span>
        <button
          type="button"
          class="sidebar-toggle"
          aria-label={props.sidebarVisible ? "Hide sidebar" : "Show sidebar"}
          aria-pressed={String(props.sidebarVisible)}
          title={props.sidebarVisible ? "Hide sidebar (⌘\\)" : "Show sidebar (⌘\\)"}
          onClick={props.onToggleSidebar}
        >
          {/* SF Symbol "sidebar.left" — same glyph macOS uses (Finder,
              Mail, Safari) for this exact action. Rendered inline so the
              webview doesn't need access to the system symbol font. */}
          <svg
            width="17"
            height="13"
            viewBox="0 0 17 13"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
            aria-hidden="true"
          >
            <rect
              x="0.85"
              y="0.85"
              width="15.3"
              height="11.3"
              rx="2.2"
              stroke="currentColor"
              stroke-width="1.3"
            />
            <line
              x1="5.6"
              y1="1.5"
              x2="5.6"
              y2="11.5"
              stroke="currentColor"
              stroke-width="1.3"
              stroke-linecap="round"
            />
            <Show when={props.sidebarVisible}>
              <rect x="1.5" y="1.5" width="3.4" height="10" fill="currentColor" opacity="0.35" />
            </Show>
          </svg>
        </button>
        <button
          type="button"
          class="workspace-pill"
          data-empty={String(!props.workspace.path)}
          title={props.workspace.path ?? "Pick a workspace folder"}
          onClick={props.onPickWorkspace}
        >
          {label()}
        </button>
        <div class="titlebar-spacer">
          <Show when={props.workspace.stale}>
            <span style={{ color: "var(--danger)", "font-size": "11px" }}>
              workspace path is stale
            </span>
          </Show>
        </div>
        <Show when={props.contextPct !== null}>
          <ContextMeter
            pct={props.contextPct ?? 0}
            tokens={props.contextTokens}
          />
        </Show>
        <button
          type="button"
          class="stop-button"
          disabled={!props.turnInFlight}
          onClick={props.onStop}
        >
          Stop
        </button>
      </div>
    </header>
  );
};

const ContextMeter: Component<{
  pct: number;
  tokens: { input: number; limit: number } | null;
}> = (props) => {
  const tone = (): "ok" | "warn" | "danger" => {
    if (props.pct >= 80) return "danger";
    if (props.pct >= 50) return "warn";
    return "ok";
  };
  const tooltip = () => {
    const t = props.tokens;
    if (!t) return `Context usage: ${props.pct}%`;
    return `Context usage: ${props.pct}% — ${t.input.toLocaleString()} / ${t.limit.toLocaleString()} tokens`;
  };
  return (
    <div class="titlebar-context" title={tooltip()}>
      <span class="titlebar-context-label">ctx</span>
      <div class="titlebar-context-bar">
        <div
          class="titlebar-context-fill"
          data-tone={tone()}
          style={{ width: `${Math.min(100, props.pct)}%` }}
        />
      </div>
      <span class="titlebar-context-value">{props.pct}%</span>
    </div>
  );
};

export default Titlebar;
