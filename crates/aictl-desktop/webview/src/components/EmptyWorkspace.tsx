import type { Component } from "solid-js";
import { Show, createSignal, onMount } from "solid-js";

import { ipc, type WorkspaceState } from "../lib/ipc";

interface Props {
  workspace: WorkspaceState;
  onPick: () => void;
  onUseDefault: () => void;
  onOpenSettings?: () => void;
}

const EmptyWorkspace: Component<Props> = (props) => {
  const [defaultPath, setDefaultPath] = createSignal<string | null>(null);

  onMount(() => {
    void ipc
      .defaultWorkspacePath()
      .then((p) => setDefaultPath(p))
      .catch(() => setDefaultPath(null));
  });

  return (
    <section class="empty-state">
      <h1>Pick a workspace</h1>
      <p>
        aictl-desktop runs every tool call inside a folder you choose —
        a project root, a scratch directory, anywhere you'd be
        comfortable with the agent reading and writing files. The
        composer unlocks once a workspace is selected.
      </p>
      <p>
        First time? Use the default workspace at{" "}
        <code>{defaultPath() ?? "~/.aictl/workspace"}</code> — it gets
        created next to your <code>~/.aictl/</code> config so the agent
        has a sandbox of its own. You can switch to a real project
        folder any time from Settings → Workspace.
      </p>
      <Show when={props.workspace.error}>
        <p style={{ color: "var(--danger)" }}>{props.workspace.error}</p>
      </Show>
      <div class="empty-state__actions">
        <button type="button" onClick={props.onUseDefault}>
          Use default workspace
        </button>
        <button
          type="button"
          class="empty-state__secondary"
          onClick={props.onPick}
        >
          Choose folder…
        </button>
      </div>
      <p class="empty-state__hint">
        Heads up: aictl-desktop needs at least one provider API key to
        talk to a model. Open{" "}
        <Show
          when={props.onOpenSettings}
          fallback={<strong>Settings</strong>}
        >
          <a
            href="#"
            onClick={(e) => {
              e.preventDefault();
              props.onOpenSettings?.();
            }}
          >
            Settings
          </a>
        </Show>
        {" "}→ API keys before sending your first message.
      </p>
    </section>
  );
};

export default EmptyWorkspace;
