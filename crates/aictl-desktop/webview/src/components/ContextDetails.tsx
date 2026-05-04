import { Show, createResource, createSignal, onCleanup, onMount } from "solid-js";
import type { Component } from "solid-js";

import { ipc, type ContextStatus } from "../lib/ipc";

interface Props {
  onClose: () => void;
}

const tone = (pct: number): "ok" | "warn" | "danger" => {
  if (pct >= 80) return "danger";
  if (pct >= 50) return "warn";
  return "ok";
};

const fmt = (n: number) => n.toLocaleString();

const ContextDetails: Component<Props> = (props) => {
  const [ctx, { refetch }] = createResource<ContextStatus>(() =>
    ipc.contextStatus(),
  );
  const [error, setError] = createSignal<string | null>(null);

  const refresh = async () => {
    setError(null);
    try {
      await refetch();
    } catch (err) {
      setError(`${err}`);
    }
  };

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

  return (
    <div
      class="ctx-details-overlay"
      role="dialog"
      aria-modal="true"
      onClick={onBackdropClick}
    >
      <div class="ctx-details-panel">
        <header class="ctx-details-header">
          <h2>Context</h2>
          <button
            type="button"
            class="ctx-details-close"
            aria-label="Close context details"
            title="Close (Esc)"
            onClick={props.onClose}
          >
            ✕
          </button>
        </header>
        <div class="ctx-details-body">
          <p class="ctx-details-hint">
            Live state of the active conversation: how full the model's
            context window is, how many messages have piled up, and where
            the auto-compact threshold sits.
          </p>
          <Show when={error()}>
            <p class="ctx-details-error">{error()}</p>
          </Show>
          <Show
            when={ctx()}
            fallback={<p class="ctx-details-meta">Loading…</p>}
          >
            {(c) => (
              <>
                <div class="ctx-details-row ctx-details-row-stack">
                  <label>Active model</label>
                  <div class="ctx-details-value">
                    <Show
                      when={c().model}
                      fallback={
                        <span class="ctx-details-empty">
                          No model selected.
                        </span>
                      }
                    >
                      <code>
                        {c().provider ?? "?"} · {c().model}
                      </code>
                    </Show>
                  </div>
                </div>
                <div class="ctx-details-row ctx-details-row-stack">
                  <label>Context window</label>
                  <div class="ctx-details-bar">
                    <div
                      class="ctx-details-fill"
                      data-tone={tone(c().context_pct)}
                      style={{ width: `${Math.min(c().context_pct, 100)}%` }}
                    />
                  </div>
                  <p class="ctx-details-meta">
                    {c().context_pct}% used — token usage {c().token_pct}% ·
                    message buffer {c().message_pct}%
                  </p>
                </div>
                <div class="ctx-details-row">
                  <label>Last input tokens</label>
                  <div class="ctx-details-value">
                    <code>
                      {fmt(c().last_input_tokens)} / {fmt(c().context_limit)}
                    </code>
                  </div>
                </div>
                <div class="ctx-details-row">
                  <label>Last output tokens</label>
                  <div class="ctx-details-value">
                    <code>{fmt(c().last_output_tokens)}</code>
                  </div>
                </div>
                <div class="ctx-details-row">
                  <label>Messages</label>
                  <div class="ctx-details-value">
                    <code>
                      {c().messages} / {c().max_messages}
                    </code>
                  </div>
                </div>
                <div class="ctx-details-row">
                  <label>Auto-compact at</label>
                  <div class="ctx-details-value">
                    <code>{c().auto_compact_threshold}%</code>{" "}
                    <span class="ctx-details-meta-inline">
                      ({c().auto_compact_overridden ? "overridden" : "default"})
                    </span>
                  </div>
                </div>
                <Show when={c().last_input_tokens === 0}>
                  <p class="ctx-details-hint">
                    <em>
                      No turns recorded yet — token counts populate after the
                      first model response.
                    </em>
                  </p>
                </Show>
              </>
            )}
          </Show>
        </div>
        <footer class="ctx-details-footer">
          <button type="button" onClick={() => void refresh()}>
            Refresh
          </button>
          <button type="button" onClick={props.onClose}>
            Close
          </button>
        </footer>
      </div>
    </div>
  );
};

export default ContextDetails;
