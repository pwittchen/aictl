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
  onShowContextDetails: () => void;
  /// Newest upstream version when an update is available, or `null`
  /// when the running build is already current / the user dismissed
  /// the badge / the probe failed.
  updateAvailable: string | null;
  onShowUpdate: () => void;
  onDismissUpdate: () => void;
  /// File-pane toggle. Hidden until a workspace is picked because the
  /// pane has nothing to show without one.
  filesVisible: boolean;
  onToggleFiles: () => void;
  onOpenSettings: () => void;
  onOpenStats: () => void;
  onOpenAbout: () => void;
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
        <Show when={props.workspace.path}>
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
        </Show>
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
        <Show when={props.updateAvailable}>
          {(version) => (
            <span class="titlebar-update" role="group" aria-label="Update available">
              <button
                type="button"
                class="titlebar-update-btn"
                title={`aictl ${version()} is available — click to view in About`}
                onClick={props.onShowUpdate}
              >
                <span class="titlebar-update-dot" aria-hidden="true" />
                <span>Update {version()}</span>
              </button>
              <button
                type="button"
                class="titlebar-update-dismiss"
                aria-label="Dismiss update notice"
                title="Dismiss until next release"
                onClick={props.onDismissUpdate}
              >
                ✕
              </button>
            </span>
          )}
        </Show>
        <Show when={props.contextPct !== null}>
          <ContextMeter
            pct={props.contextPct ?? 0}
            tokens={props.contextTokens}
            onClick={props.onShowContextDetails}
          />
        </Show>
        <button
          type="button"
          class="info-toggle"
          aria-label="About aictl"
          title="About aictl"
          onClick={props.onOpenAbout}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 16 16"
            fill="currentColor"
            width="16"
            height="16"
            aria-hidden="true"
          >
            <path
              fill-rule="evenodd"
              d="M15 8A7 7 0 1 1 1 8a7 7 0 0 1 14 0ZM9 5a1 1 0 1 1-2 0 1 1 0 0 1 2 0ZM6.75 8a.75.75 0 0 0 0 1.5h.75v1.75a.75.75 0 0 0 1.5 0v-2.5A.75.75 0 0 0 8.25 8h-1.5Z"
              clip-rule="evenodd"
            />
          </svg>
        </button>
        <button
          type="button"
          class="stats-toggle"
          aria-label="Open stats"
          title="Open stats"
          onClick={props.onOpenStats}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 16 16"
            fill="currentColor"
            width="16"
            height="16"
            aria-hidden="true"
          >
            <path d="M12 2a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h1a1 1 0 0 0 1-1V3a1 1 0 0 0-1-1h-1ZM6.5 6a1 1 0 0 1 1-1h1a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1h-1a1 1 0 0 1-1-1V6ZM2 9a1 1 0 0 1 1-1h1a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V9Z" />
          </svg>
        </button>
        <button
          type="button"
          class="settings-toggle"
          aria-label="Open settings"
          title="Open settings (⌘,)"
          onClick={props.onOpenSettings}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 16 16"
            fill="currentColor"
            width="16"
            height="16"
            aria-hidden="true"
          >
            <path d="M6.5 2.25a.75.75 0 0 0-1.5 0v3a.75.75 0 0 0 1.5 0V4.5h6.75a.75.75 0 0 0 0-1.5H6.5v-.75ZM11 6.5a.75.75 0 0 0-1.5 0v3a.75.75 0 0 0 1.5 0v-.75h2.25a.75.75 0 0 0 0-1.5H11V6.5ZM5.75 10a.75.75 0 0 1 .75.75v.75h6.75a.75.75 0 0 1 0 1.5H6.5v.75a.75.75 0 0 1-1.5 0v-3a.75.75 0 0 1 .75-.75ZM2.75 7.25H8.5v1.5H2.75a.75.75 0 0 1 0-1.5ZM4 3H2.75a.75.75 0 0 0 0 1.5H4V3ZM2.75 11.5H4V13H2.75a.75.75 0 0 1 0-1.5Z" />
          </svg>
        </button>
        <Show when={props.workspace.path}>
          <button
            type="button"
            class="files-toggle"
            aria-label={props.filesVisible ? "Hide files pane" : "Show files pane"}
            aria-pressed={String(props.filesVisible)}
            title={props.filesVisible ? "Hide files (⌘.)" : "Show files (⌘.)"}
            onClick={props.onToggleFiles}
          >
            <Show
              when={props.filesVisible}
              fallback={
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  viewBox="0 0 16 16"
                  fill="currentColor"
                  width="16"
                  height="16"
                  aria-hidden="true"
                >
                  <path d="M2 3.5A1.5 1.5 0 0 1 3.5 2h2.879a1.5 1.5 0 0 1 1.06.44l1.122 1.12A1.5 1.5 0 0 0 9.62 4H12.5A1.5 1.5 0 0 1 14 5.5v1.401a2.986 2.986 0 0 0-1.5-.401h-9c-.546 0-1.059.146-1.5.401V3.5ZM2 9.5v3A1.5 1.5 0 0 0 3.5 14h9a1.5 1.5 0 0 0 1.5-1.5v-3A1.5 1.5 0 0 0 12.5 8h-9A1.5 1.5 0 0 0 2 9.5Z" />
                </svg>
              }
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                viewBox="0 0 16 16"
                fill="currentColor"
                width="16"
                height="16"
                aria-hidden="true"
              >
                <path d="M3 3.5A1.5 1.5 0 0 1 4.5 2h1.879a1.5 1.5 0 0 1 1.06.44l1.122 1.12A1.5 1.5 0 0 0 9.62 4H11.5A1.5 1.5 0 0 1 13 5.5v1H3v-3ZM3.081 8a1.5 1.5 0 0 0-1.423 1.974l1 3A1.5 1.5 0 0 0 4.081 14h7.838a1.5 1.5 0 0 0 1.423-1.026l1-3A1.5 1.5 0 0 0 12.919 8H3.081Z" />
              </svg>
            </Show>
          </button>
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
  onClick: () => void;
}> = (props) => {
  const tone = (): "ok" | "warn" | "danger" => {
    if (props.pct >= 80) return "danger";
    if (props.pct >= 50) return "warn";
    return "ok";
  };
  const tooltip = () => {
    const t = props.tokens;
    const suffix = "click for details";
    if (!t) return `Context usage: ${props.pct}% — ${suffix}`;
    return `Context usage: ${props.pct}% — ${t.input.toLocaleString()} / ${t.limit.toLocaleString()} tokens — ${suffix}`;
  };
  return (
    <button
      type="button"
      class="titlebar-context"
      title={tooltip()}
      aria-label="Show context details"
      onClick={props.onClick}
    >
      <span class="titlebar-context-label">ctx</span>
      <div class="titlebar-context-bar">
        <div
          class="titlebar-context-fill"
          data-tone={tone()}
          style={{ width: `${Math.min(100, props.pct)}%` }}
        />
      </div>
      <span class="titlebar-context-value">{props.pct}%</span>
    </button>
  );
};

export default Titlebar;
