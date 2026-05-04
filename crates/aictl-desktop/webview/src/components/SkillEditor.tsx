import { Show, createSignal, onCleanup, onMount } from "solid-js";
import type { Component } from "solid-js";

import { ipc } from "../lib/ipc";

interface Props {
  /// Names of currently-installed skills — used to surface the
  /// overwrite confirmation before the backend rejects the save.
  existingNames: string[];
  onSaved: (name: string) => void;
  onClose: () => void;
}

type Mode = "manual" | "ai";

const SkillEditor: Component<Props> = (props) => {
  const [mode, setMode] = createSignal<Mode>("manual");
  const [name, setName] = createSignal("");
  // Description doubles as: (a) the frontmatter `description` written
  // to disk, and (b) in AI mode, the seed text fed to the generator.
  const [description, setDescription] = createSignal("");
  const [body, setBody] = createSignal("");
  const [generating, setGenerating] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [info, setInfo] = createSignal<string | null>(null);

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      props.onClose();
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
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
      setError("Describe what the skill should do.");
      return;
    }
    setGenerating(true);
    try {
      const text = await ipc.skillGenerate(name().trim(), description().trim());
      setBody(text);
      setInfo("body generated — review and Save");
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
    if (description().trim() === "") {
      setError("Skill description is required.");
      return;
    }
    if (body().trim() === "") {
      setError("Skill body is empty.");
      return;
    }
    if (clash()) {
      const ok = window.confirm(
        `A skill named "${name().trim()}" already exists. Overwrite it?`,
      );
      if (!ok) return;
    }
    setSaving(true);
    try {
      await ipc.skillSave(
        name().trim(),
        description().trim(),
        body(),
        clash(),
      );
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
          <h2>New Skill</h2>
          <button
            type="button"
            class="editor-modal-close"
            aria-label="Close new-skill dialog"
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
            <label for="skill-editor-name">Name</label>
            <input
              id="skill-editor-name"
              type="text"
              placeholder="my-skill"
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
                A skill with this name already exists — saving will
                prompt to overwrite.
              </p>
            </Show>
          </div>
          <div class="editor-modal-row">
            <label for="skill-editor-desc">Description</label>
            <input
              id="skill-editor-desc"
              type="text"
              placeholder="one-line summary shown in pickers"
              value={description()}
              onInput={(e) => setDescription(e.currentTarget.value)}
            />
            <p class="editor-modal-help">
              {mode() === "ai"
                ? "Saved as the SKILL.md frontmatter description and used as the seed for the AI generator."
                : "Saved as the SKILL.md frontmatter description."}
            </p>
            <Show when={mode() === "ai"}>
              <div class="editor-modal-actions inline">
                <button
                  type="button"
                  disabled={generating() || saving()}
                  onClick={() => void generate()}
                >
                  {generating() ? "Generating…" : "Generate"}
                </button>
              </div>
            </Show>
          </div>
          <div class="editor-modal-row">
            <label for="skill-editor-body">Body</label>
            <textarea
              id="skill-editor-body"
              rows={mode() === "ai" ? 12 : 16}
              placeholder={
                mode() === "ai"
                  ? "(generated body appears here — editable before save)"
                  : "Numbered steps the assistant should follow when this skill is invoked…"
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

export default SkillEditor;
