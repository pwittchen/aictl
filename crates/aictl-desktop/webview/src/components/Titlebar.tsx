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

export default Titlebar;
