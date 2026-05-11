import type { Component } from "solid-js";
import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";

interface Props {
  mode: "file" | "dir";
  /// Workspace-relative directory the new entry will land in. Empty
  /// string ("") = workspace root.
  base: string;
  /// Surfaced inline once the user submits — `null` clears the row.
  /// The parent re-passes the latest error so a follow-up rejection
  /// (already-exists, invalid name) replaces the previous text.
  error: string | null;
  onSubmit: (name: string) => void;
  onCancel: () => void;
}

/// Centered modal for creating a new file or directory inside the
/// workspace. Reuses the `.tool-modal` shell so it visually matches the
/// existing tool-approval prompt — same dim backdrop, same bordered
/// panel, same uppercase-mono header.
const CreatePrompt: Component<Props> = (props) => {
  const [name, setName] = createSignal("");
  let inputRef: HTMLInputElement | undefined;

  const title = () => (props.mode === "file" ? "New file" : "New directory");
  const target = () => (props.base === "" ? "workspace root" : props.base);

  const submit = () => {
    const trimmed = name().trim();
    if (!trimmed) return;
    props.onSubmit(trimmed);
  };

  // Capture-phase + stopImmediatePropagation so any parent overlay
  // listening for Esc/Enter on window doesn't fire alongside this one
  // and close the surrounding UI underneath the modal.
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopImmediatePropagation();
      submit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      props.onCancel();
    }
  };

  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
    // Defer focus so the input is mounted before we touch it.
    queueMicrotask(() => inputRef?.focus());
  });

  // Clear the typed name when the prompt switches modes (file → dir).
  createEffect(() => {
    void props.mode;
    setName("");
  });

  return (
    <div class="tool-modal" role="dialog" aria-modal="true">
      <div class="panel">
        <h2>{title()}</h2>
        <div class="create-prompt-target">
          create in <code>{target()}</code>
        </div>
        <input
          ref={inputRef}
          type="text"
          class="create-prompt-input"
          value={name()}
          placeholder={
            props.mode === "file" ? "filename" : "directory name"
          }
          onInput={(e) => setName(e.currentTarget.value)}
        />
        <Show when={props.error}>
          <div class="create-prompt-error">{props.error}</div>
        </Show>
        <div class="actions">
          <button type="button" data-variant="deny" onClick={props.onCancel}>
            Cancel Esc
          </button>
          <button
            type="button"
            data-variant="allow"
            disabled={name().trim() === ""}
            onClick={submit}
          >
            Create ↩
          </button>
        </div>
      </div>
    </div>
  );
};

export default CreatePrompt;
