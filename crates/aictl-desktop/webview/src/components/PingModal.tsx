import type { Component } from "solid-js";
import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";

import { ipc, type PingResult, type PingStatus } from "../lib/ipc";

interface Props {
  disabled: boolean;
}

const PROVIDER_LABELS: Record<string, string> = {
  anthropic: "Anthropic",
  openai: "OpenAI",
  gemini: "Gemini",
  grok: "Grok",
  mistral: "Mistral",
  deepseek: "DeepSeek",
  kimi: "Kimi",
  zai: "Z.ai",
  ollama: "Ollama",
  "aictl-server": "aictl-server",
};

const PingIcon: Component = () => (
  <svg
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 16 16"
    fill="currentColor"
    aria-hidden="true"
  >
    <path
      fill-rule="evenodd"
      d="M13.78 10.47a.75.75 0 0 1 0 1.06l-2.25 2.25a.75.75 0 0 1-1.06 0l-2.25-2.25a.75.75 0 1 1 1.06-1.06l.97.97V5.75a.75.75 0 0 1 1.5 0v5.69l.97-.97a.75.75 0 0 1 1.06 0ZM2.22 5.53a.75.75 0 0 1 0-1.06l2.25-2.25a.75.75 0 0 1 1.06 0l2.25 2.25a.75.75 0 0 1-1.06 1.06l-.97-.97v5.69a.75.75 0 0 1-1.5 0V4.56l-.97.97a.75.75 0 0 1-1.06 0Z"
      clip-rule="evenodd"
    />
  </svg>
);

const CheckIcon: Component = () => (
  <svg
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 16 16"
    fill="currentColor"
    aria-hidden="true"
  >
    <path
      fill-rule="evenodd"
      d="M12.416 3.376a.75.75 0 0 1 .208 1.04l-5 7.5a.75.75 0 0 1-1.154.114l-3-3a.75.75 0 0 1 1.06-1.06l2.353 2.353 4.493-6.74a.75.75 0 0 1 1.04-.207Z"
      clip-rule="evenodd"
    />
  </svg>
);

const XIcon: Component = () => (
  <svg
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 16 16"
    fill="currentColor"
    aria-hidden="true"
  >
    <path d="M5.28 4.22a.75.75 0 0 0-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 1 0 1.06 1.06L8 9.06l2.72 2.72a.75.75 0 1 0 1.06-1.06L9.06 8l2.72-2.72a.75.75 0 0 0-1.06-1.06L8 6.94 5.28 4.22Z" />
  </svg>
);

const rowState = (s: PingStatus): "ok" | "warn" | "error" => {
  switch (s) {
    case "ok":
      return "ok";
    case "no_key":
      return "warn";
    case "fail":
    case "not_running":
      return "error";
  }
};

const labelFor = (provider: string): string =>
  PROVIDER_LABELS[provider] ?? provider;

const PingModal: Component<Props> = (props) => {
  const [open, setOpen] = createSignal(false);
  const [results, setResults] = createSignal<PingResult[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const runProbe = async () => {
    setLoading(true);
    setError(null);
    try {
      const r = await ipc.pingProviders();
      setResults(r);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && open()) {
      e.preventDefault();
      e.stopImmediatePropagation();
      setOpen(false);
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
  });

  const onBackdrop = (e: MouseEvent) => {
    if (e.target === e.currentTarget) setOpen(false);
  };

  const openModal = () => {
    setOpen(true);
    void runProbe();
  };

  const summary = () => {
    const r = results();
    if (r.length === 0) return null;
    const ok = r.filter((x) => x.status === "ok").length;
    const fail = r.filter(
      (x) => x.status === "fail" || x.status === "not_running",
    ).length;
    const skipped = r.filter((x) => x.status === "no_key").length;
    return `${ok} ok · ${fail} fail · ${skipped} no key`;
  };

  return (
    <>
      <button
        type="button"
        class="ping-icon"
        disabled={props.disabled}
        aria-label="Ping providers — click for connection status"
        title="ping providers — click for connection status"
        onClick={openModal}
      >
        <PingIcon />
      </button>
      <Show when={open()}>
        <Portal mount={document.body}>
          <div
            class="security-modal-overlay"
            role="dialog"
            aria-modal="true"
            onClick={onBackdrop}
          >
            <div class="security-modal-panel">
              <header class="security-modal-header">
                <h2>Provider Ping</h2>
                <button
                  type="button"
                  class="security-modal-close"
                  aria-label="Close ping details"
                  title="Close (Esc)"
                  onClick={() => setOpen(false)}
                >
                  ✕
                </button>
              </header>
              <div class="security-modal-body">
                <Show when={error()}>
                  <div class="security-modal-banner" data-state="error">
                    <div class="security-modal-banner-text">
                      <div class="security-modal-banner-title">Probe failed</div>
                      <p>{error()}</p>
                    </div>
                  </div>
                </Show>
                <Show when={!error() && summary()}>
                  <div class="security-modal-banner" data-state="ok">
                    <div class="security-modal-banner-text">
                      <div class="security-modal-banner-title">Summary</div>
                      <p>{summary()}</p>
                    </div>
                  </div>
                </Show>
                <Show
                  when={!loading() || results().length > 0}
                  fallback={
                    <div class="ping-modal-loading">Pinging providers…</div>
                  }
                >
                  <ul class="security-modal-checks">
                    <For each={results()}>
                      {(r) => (
                        <li
                          class="security-modal-check"
                          data-state={rowState(r.status)}
                        >
                          <span class="security-modal-check-icon">
                            {r.status === "ok" ? <CheckIcon /> : <XIcon />}
                          </span>
                          <span class="security-modal-check-label">
                            <span class="ping-modal-row">
                              <span class="ping-modal-provider">
                                {labelFor(r.provider)}
                              </span>
                              <span class="ping-modal-detail">{r.detail}</span>
                              <Show when={r.elapsed_ms !== null}>
                                <span class="ping-modal-elapsed">
                                  {r.elapsed_ms}ms
                                </span>
                              </Show>
                            </span>
                          </span>
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
              </div>
              <footer class="security-modal-footer">
                <button type="button" onClick={() => setOpen(false)}>
                  Close
                </button>
                <button
                  type="button"
                  class="security-modal-primary"
                  disabled={loading()}
                  onClick={() => void runProbe()}
                >
                  {loading() ? "Pinging…" : "Re-ping"}
                </button>
              </footer>
            </div>
          </div>
        </Portal>
      </Show>
    </>
  );
};

export default PingModal;
