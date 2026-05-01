import type { Component } from "solid-js";
import { onCleanup, onMount } from "solid-js";

import type { PendingApproval } from "../App";

interface Props {
  request: PendingApproval;
  onAllow: () => void;
  onDeny: () => void;
  onAlways: () => void;
}

const ToolApproval: Component<Props> = (props) => {
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      props.onAllow();
    } else if (e.key === "Escape") {
      e.preventDefault();
      props.onDeny();
    } else if (e.key === "a" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      props.onAlways();
    }
  };

  onMount(() => {
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <div class="tool-modal" role="dialog" aria-modal="true">
      <div class="panel">
        <h2>tool · {props.request.tool}</h2>
        <pre>{props.request.input || "(empty body)"}</pre>
        <div class="actions">
          <button type="button" onClick={props.onAlways}>
            Always allow ⌘A
          </button>
          <button type="button" data-variant="deny" onClick={props.onDeny}>
            Deny Esc
          </button>
          <button type="button" data-variant="allow" onClick={props.onAllow}>
            Allow ↩
          </button>
        </div>
      </div>
    </div>
  );
};

export default ToolApproval;
