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
  // Capture-phase + stopImmediatePropagation so any parent overlay
  // listening for Esc/Enter on window doesn't fire alongside this one
  // and close the surrounding UI underneath the modal.
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopImmediatePropagation();
      props.onAllow();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      props.onDeny();
    } else if (e.key === "a" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      e.stopImmediatePropagation();
      props.onAlways();
    }
  };

  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
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
