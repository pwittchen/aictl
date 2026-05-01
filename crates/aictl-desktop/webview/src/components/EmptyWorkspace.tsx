import type { Component } from "solid-js";
import { Show } from "solid-js";

import type { WorkspaceState } from "../lib/ipc";

interface Props {
  workspace: WorkspaceState;
  onPick: () => void;
}

const EmptyWorkspace: Component<Props> = (props) => {
  return (
    <section class="empty-state">
      <h1>Pick a workspace</h1>
      <p>
        aictl-desktop runs every tool call inside a folder you choose —
        a project root, a scratch directory, anywhere you'd be
        comfortable with the agent reading and writing files. The
        composer unlocks once a workspace is selected.
      </p>
      <Show when={props.workspace.error}>
        <p style={{ color: "var(--danger)" }}>{props.workspace.error}</p>
      </Show>
      <button type="button" onClick={props.onPick}>
        Choose workspace folder…
      </button>
    </section>
  );
};

export default EmptyWorkspace;
