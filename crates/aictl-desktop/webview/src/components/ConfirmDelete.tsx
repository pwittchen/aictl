import type { Component } from "solid-js";
import { onCleanup, onMount } from "solid-js";

interface Props {
  /// Header line — e.g. "Delete session", "Delete file".
  title: string;
  /// Free-form summary of *what* will be deleted, rendered in a mono
  /// `<pre>` block. Mirrors the tool-approval body slot so the visual
  /// rhythm matches.
  detail: string;
  /// Optional second line under the detail (extra warning, file path,
  /// row count, etc.). Skipped when null.
  note?: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}

/// Centered modal asking the user to confirm a destructive action.
/// Reuses the `.tool-modal` shell so the visual language matches the
/// tool-approval prompt and the create-file prompt — same dim backdrop,
/// same bordered panel, same uppercase-mono header.
const ConfirmDelete: Component<Props> = (props) => {
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      props.onConfirm();
    } else if (e.key === "Escape") {
      e.preventDefault();
      props.onCancel();
    }
  };

  onMount(() => {
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <div class="tool-modal" role="dialog" aria-modal="true">
      <div class="panel">
        <h2>{props.title}</h2>
        <pre>{props.detail}</pre>
        {props.note ? <div class="confirm-delete-note">{props.note}</div> : null}
        <div class="actions">
          <button type="button" onClick={props.onCancel}>
            Cancel Esc
          </button>
          <button type="button" data-variant="deny" onClick={props.onConfirm}>
            Delete ↩
          </button>
        </div>
      </div>
    </div>
  );
};

export default ConfirmDelete;
