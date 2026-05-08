import type { Component } from "solid-js";
import { Match, Show, Switch, createEffect, createSignal, onCleanup } from "solid-js";

import { ipc, type FileContents } from "../lib/ipc";

interface Props {
  /// Workspace-relative file path the user picked from the tree. The
  /// pane re-loads whenever this changes.
  path: string;
  /// Filesystem-watcher pulse from App. Each bump prompts the editor to
  /// re-read its file: external edits propagate in, deletions close the
  /// pane.
  fsTick: number;
  onClose: () => void;
}

interface OpenFile {
  path: string;
  /// Last-saved contents — used to detect dirty state without storing
  /// the buffer on every keystroke.
  saved: string;
  size_bytes: number;
}

const EditorPane: Component<Props> = (props) => {
  const [open, setOpen] = createSignal<OpenFile | null>(null);
  const [buffer, setBuffer] = createSignal<string>("");
  const [openError, setOpenError] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [saveStatus, setSaveStatus] = createSignal<string | null>(null);

  const dirty = () => {
    const cur = open();
    return cur !== null && buffer() !== cur.saved;
  };

  const load = async (rel: string) => {
    setOpenError(null);
    setSaveStatus(null);
    try {
      const result: FileContents = await ipc.workspaceReadFile(rel);
      setOpen({
        path: result.path,
        saved: result.contents,
        size_bytes: result.size_bytes,
      });
      setBuffer(result.contents);
    } catch (err) {
      setOpen(null);
      setBuffer("");
      setOpenError(`${err}`);
    }
  };

  /// Re-read the file in response to a fs-watcher pulse. Three outcomes:
  ///   * file is gone — close the pane (the user's tree row will vanish
  ///     too on the next FilePane refresh; keeping the editor open on a
  ///     dead path would be misleading)
  ///   * buffer was clean — silently sync to the new contents so an
  ///     external edit propagates in
  ///   * buffer was dirty — only update `saved` so the dirty dot remains
  ///     accurate; the user keeps their unsaved edits and can decide
  ///     whether to save over the new on-disk version
  const refresh = async () => {
    const cur = open();
    if (!cur) return;
    try {
      const result: FileContents = await ipc.workspaceReadFile(cur.path);
      const wasDirty = buffer() !== cur.saved;
      setOpen({
        path: result.path,
        saved: result.contents,
        size_bytes: result.size_bytes,
      });
      if (!wasDirty) {
        setBuffer(result.contents);
      }
    } catch {
      // The file is gone (or no longer readable as text). Close the
      // editor so the user isn't stranded on a stale buffer.
      props.onClose();
    }
  };

  const save = async () => {
    const cur = open();
    if (!cur || saving()) return;
    setSaving(true);
    setSaveStatus(null);
    try {
      const result = await ipc.workspaceWriteFile(cur.path, buffer());
      setOpen({
        path: result.path,
        saved: result.contents,
        size_bytes: result.size_bytes,
      });
      setSaveStatus("Saved");
      window.setTimeout(() => setSaveStatus(null), 1600);
    } catch (err) {
      setSaveStatus(`${err}`);
    } finally {
      setSaving(false);
    }
  };

  const revert = () => {
    const cur = open();
    if (!cur) return;
    setBuffer(cur.saved);
    setSaveStatus(null);
  };

  // Load whenever the path prop changes.
  createEffect(() => {
    const p = props.path;
    if (!p) return;
    void load(p);
  });

  // Refresh on every fs-watcher pulse after the initial load. We track
  // both the initial path tick and a separate fs tick so opening a new
  // file doesn't double-fire (the load above already covers that case).
  let lastFsTick = -1;
  createEffect(() => {
    const tick = props.fsTick;
    if (lastFsTick === -1) {
      lastFsTick = tick;
      return;
    }
    if (tick === lastFsTick) return;
    lastFsTick = tick;
    void refresh();
  });

  // ⌘S / Ctrl-S saves the active file. The textarea swallows keydown,
  // so we listen on the window for the shortcut.
  const onKey = (e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
      if (!open()) return;
      e.preventDefault();
      void save();
    }
  };
  window.addEventListener("keydown", onKey);
  onCleanup(() => window.removeEventListener("keydown", onKey));

  return (
    <aside class="editor-pane" aria-label="File editor">
      <header class="editor-pane-header">
        <span class="editor-pane-title" title={props.path}>
          {props.path}
          <Show when={dirty()}>
            <span class="file-pane-dot" aria-label="Unsaved changes">
              •
            </span>
          </Show>
        </span>
        <span class="editor-pane-meta">
          {saveStatus() ?? <Show when={open()}>{(f) => `${f().size_bytes} B`}</Show>}
        </span>
        <button
          type="button"
          class="editor-pane-icon"
          aria-label="Revert changes"
          title="Revert"
          onClick={revert}
          disabled={!dirty() || saving()}
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
              clip-rule="evenodd"
              d="M12.5 9.75A2.75 2.75 0 0 0 9.75 7H4.56l2.22 2.22a.75.75 0 1 1-1.06 1.06l-3.5-3.5a.75.75 0 0 1 0-1.06l3.5-3.5a.75.75 0 0 1 1.06 1.06L4.56 5.5h5.19a4.25 4.25 0 0 1 0 8.5h-1a.75.75 0 0 1 0-1.5h1a2.75 2.75 0 0 0 2.75-2.75Z"
            />
          </svg>
        </button>
        <button
          type="button"
          class="editor-pane-icon"
          aria-label={saving() ? "Saving" : "Save (⌘S)"}
          title={saving() ? "Saving…" : "Save (⌘S)"}
          onClick={save}
          disabled={!dirty() || saving()}
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            viewBox="0 0 24 24"
            fill="currentColor"
            width="16"
            height="16"
            aria-hidden="true"
          >
            <path
              fill-rule="evenodd"
              clip-rule="evenodd"
              d="M18.1716 1C18.702 1 19.2107 1.21071 19.5858 1.58579L22.4142 4.41421C22.7893 4.78929 23 5.29799 23 5.82843V20C23 21.6569 21.6569 23 20 23H4C2.34315 23 1 21.6569 1 20V4C1 2.34315 2.34315 1 4 1H18.1716ZM4 3C3.44772 3 3 3.44772 3 4V20C3 20.5523 3.44772 21 4 21L5 21L5 15C5 13.3431 6.34315 12 8 12L16 12C17.6569 12 19 13.3431 19 15V21H20C20.5523 21 21 20.5523 21 20V6.82843C21 6.29799 20.7893 5.78929 20.4142 5.41421L18.5858 3.58579C18.2107 3.21071 17.702 3 17.1716 3H17V5C17 6.65685 15.6569 8 14 8H10C8.34315 8 7 6.65685 7 5V3H4ZM17 21V15C17 14.4477 16.5523 14 16 14L8 14C7.44772 14 7 14.4477 7 15L7 21L17 21ZM9 3H15V5C15 5.55228 14.5523 6 14 6H10C9.44772 6 9 5.55228 9 5V3Z"
            />
          </svg>
        </button>
        <button
          type="button"
          class="file-pane-close"
          aria-label="Close editor"
          title="Close editor"
          onClick={props.onClose}
        >
          ✕
        </button>
      </header>
      <Switch>
        <Match when={openError()}>
          <div class="file-pane-empty file-pane-empty-error">
            {openError()}
          </div>
        </Match>
        <Match when={open()}>
          <textarea
            class="file-pane-editor-area"
            spellcheck={false}
            value={buffer()}
            onInput={(e) =>
              setBuffer((e.currentTarget as HTMLTextAreaElement).value)
            }
          />
        </Match>
        <Match when={true}>
          <div class="file-pane-empty">Loading…</div>
        </Match>
      </Switch>
    </aside>
  );
};

export default EditorPane;
