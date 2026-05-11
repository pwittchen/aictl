import { Show, createSignal, onCleanup, onMount } from "solid-js";
import type { Component } from "solid-js";

import { ipc } from "../lib/ipc";

interface Props {
  /// Names of currently-installed agents — used to detect a clash and
  /// surface the overwrite confirmation before the backend rejects the
  /// save.
  existingNames: string[];
  onSaved: (name: string) => void;
  onClose: () => void;
}

type Mode = "manual" | "ai";

const AgentEditor: Component<Props> = (props) => {
  const [mode, setMode] = createSignal<Mode>("manual");
  const [name, setName] = createSignal("");
  const [body, setBody] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [generating, setGenerating] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [info, setInfo] = createSignal<string | null>(null);

  // Capture-phase + stopImmediatePropagation so the parent <Settings>'s
  // window-level Esc handler doesn't fire alongside this one and close
  // the whole panel underneath the editor.
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopImmediatePropagation();
      props.onClose();
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
  });

  const onBackdropClick = (e: MouseEvent) => {
    if (e.target === e.currentTarget) props.onClose();
  };

  const validName = () => /^[A-Za-z0-9_-]+$/.test(name().trim());
  const clash = () =>
    name().trim() !== "" && props.existingNames.includes(name().trim());

  const generate = async () => {
    setError(null);
    setInfo(null);
    if (!validName()) {
      setError("Invalid name — letters, numbers, underscore, or dash only.");
      return;
    }
    if (description().trim() === "") {
      setError("Describe what the agent should do.");
      return;
    }
    setGenerating(true);
    try {
      const text = await ipc.agentGenerate(name().trim(), description().trim());
      setBody(text);
      setInfo("prompt generated — review and Save");
    } catch (err) {
      setError(`${err}`);
    } finally {
      setGenerating(false);
    }
  };

  const save = async () => {
    setError(null);
    setInfo(null);
    if (!validName()) {
      setError("Invalid name — letters, numbers, underscore, or dash only.");
      return;
    }
    if (body().trim() === "") {
      setError("Agent prompt is empty.");
      return;
    }
    if (clash()) {
      const ok = window.confirm(
        `An agent named "${name().trim()}" already exists. Overwrite it?`,
      );
      if (!ok) return;
    }
    setSaving(true);
    try {
      await ipc.agentSave(name().trim(), body(), clash());
      props.onSaved(name().trim());
    } catch (err) {
      setError(`${err}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div
      class="editor-modal-overlay"
      role="dialog"
      aria-modal="true"
      onClick={onBackdropClick}
    >
      <div class="editor-modal-panel">
        <header class="editor-modal-header">
          <h2>New Agent</h2>
          <button
            type="button"
            class="editor-modal-close"
            aria-label="Close new-agent dialog"
            title="Close (Esc)"
            onClick={props.onClose}
          >
            ✕
          </button>
        </header>
        <nav class="editor-modal-tabs">
          <button
            type="button"
            class="editor-modal-tab"
            data-active={String(mode() === "manual")}
            onClick={() => setMode("manual")}
          >
            Manual
          </button>
          <button
            type="button"
            class="editor-modal-tab"
            data-active={String(mode() === "ai")}
            onClick={() => setMode("ai")}
          >
            Generate with AI
          </button>
        </nav>
        <div class="editor-modal-body">
          <Show when={error()}>
            <p class="editor-modal-error">{error()}</p>
          </Show>
          <Show when={info()}>
            <p class="editor-modal-info">{info()}</p>
          </Show>
          <div class="editor-modal-row">
            <label for="editor-modal-name">Name</label>
            <input
              id="editor-modal-name"
              type="text"
              placeholder="my-agent"
              value={name()}
              onInput={(e) => setName(e.currentTarget.value)}
            />
            <Show when={name() !== "" && !validName()}>
              <p class="editor-modal-help danger">
                Use only letters, numbers, underscore, or dash.
              </p>
            </Show>
            <Show when={validName() && clash()}>
              <p class="editor-modal-help warn">
                An agent with this name already exists — saving will
                prompt to overwrite.
              </p>
            </Show>
          </div>
          <Show when={mode() === "ai"}>
            <div class="editor-modal-row">
              <label for="editor-modal-desc">Description</label>
              <input
                id="editor-modal-desc"
                type="text"
                placeholder="what should this agent do?"
                value={description()}
                onInput={(e) => setDescription(e.currentTarget.value)}
              />
              <p class="editor-modal-help">
                Generates the prompt body using the active provider/model
                (Settings → Model). Review the result before saving.
              </p>
              <div class="editor-modal-actions inline">
                <button
                  type="button"
                  disabled={generating() || saving()}
                  onClick={() => void generate()}
                >
                  {generating() ? "Generating…" : "Generate"}
                </button>
              </div>
            </div>
          </Show>
          <div class="editor-modal-row">
            <label for="editor-modal-body">Prompt</label>
            <textarea
              id="editor-modal-body"
              rows={mode() === "ai" ? 12 : 16}
              placeholder={
                mode() === "ai"
                  ? "(generated prompt appears here — editable before save)"
                  : "Type or paste the agent's system prompt body…"
              }
              value={body()}
              onInput={(e) => setBody(e.currentTarget.value)}
            />
          </div>
        </div>
        <footer class="editor-modal-footer">
          <button
            type="button"
            disabled={saving() || generating()}
            onClick={props.onClose}
          >
            Cancel
          </button>
          <button
            type="button"
            disabled={saving() || generating()}
            onClick={() => void save()}
          >
            {saving() ? "Saving…" : "Save"}
          </button>
        </footer>
      </div>
    </div>
  );
};

export default AgentEditor;
