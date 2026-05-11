import type { Component } from "solid-js";
import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";

export type ShieldState = "ok" | "warn" | "error";

export interface ShieldCheck {
  label: string;
  ok: boolean;
  hint?: string;
}

interface Props {
  state: ShieldState;
  checks: ShieldCheck[];
  disabled: boolean;
  onOpenSettings: () => void;
}

const ShieldOk: Component = () => (
  <svg
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 16 16"
    fill="currentColor"
    aria-hidden="true"
  >
    <path
      fill-rule="evenodd"
      d="M8.5 1.709a.75.75 0 0 0-1 0 8.963 8.963 0 0 1-4.84 2.217.75.75 0 0 0-.654.72 10.499 10.499 0 0 0 5.647 9.672.75.75 0 0 0 .694-.001 10.499 10.499 0 0 0 5.647-9.672.75.75 0 0 0-.654-.719A8.963 8.963 0 0 1 8.5 1.71Zm2.34 5.504a.75.75 0 0 0-1.18-.926L7.394 9.17l-1.156-.99a.75.75 0 1 0-.976 1.138l1.75 1.5a.75.75 0 0 0 1.078-.106l2.75-3.5Z"
      clip-rule="evenodd"
    />
  </svg>
);

const ShieldAlert: Component = () => (
  <svg
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 16 16"
    fill="currentColor"
    aria-hidden="true"
  >
    <path
      fill-rule="evenodd"
      d="M7.5 1.709a.75.75 0 0 1 1 0 8.963 8.963 0 0 0 4.84 2.217.75.75 0 0 1 .654.72 10.499 10.499 0 0 1-5.647 9.672.75.75 0 0 1-.694-.001 10.499 10.499 0 0 1-5.647-9.672.75.75 0 0 1 .654-.719A8.963 8.963 0 0 0 7.5 1.71ZM8 5a.75.75 0 0 1 .75.75v2a.75.75 0 0 1-1.5 0v-2A.75.75 0 0 1 8 5Zm0 7a1 1 0 1 0 0-2 1 1 0 0 0 0 2Z"
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

const STATE_LABELS: Record<ShieldState, string> = {
  ok: "Protected",
  warn: "Attention",
  error: "Unprotected",
};

const STATE_DESCRIPTIONS: Record<ShieldState, string> = {
  ok: "All security and redaction features are on, and every API key lives in the system keyring.",
  warn: "Security is on, but some defenses are disabled. Review the items below to harden your setup.",
  error: "Security policy is off. The CWD jail, shell allow-list, and tool denial are all bypassed.",
};

const SecurityShield: Component<Props> = (props) => {
  const [open, setOpen] = createSignal(false);

  // Capture-phase + stopImmediatePropagation so other window-level Esc
  // handlers (Sidebar/Composer/FilePane) don't fire alongside this one
  // while the popover is open.
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

  const renderShield = () =>
    props.state === "ok" ? <ShieldOk /> : <ShieldAlert />;

  return (
    <>
      <button
        type="button"
        class="security-icon"
        data-state={props.state}
        disabled={props.disabled}
        aria-label={`Security status: ${STATE_LABELS[props.state]} — click for details`}
        title={`security: ${STATE_LABELS[props.state].toLowerCase()} — click for details`}
        onClick={() => setOpen(true)}
      >
        {renderShield()}
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
                <h2>Security</h2>
                <button
                  type="button"
                  class="security-modal-close"
                  aria-label="Close security details"
                  title="Close (Esc)"
                  onClick={() => setOpen(false)}
                >
                  ✕
                </button>
              </header>
              <div class="security-modal-body">
                <div class="security-modal-banner" data-state={props.state}>
                  <div class="security-modal-shield">{renderShield()}</div>
                  <div class="security-modal-banner-text">
                    <div class="security-modal-banner-title">
                      {STATE_LABELS[props.state]}
                    </div>
                    <p>{STATE_DESCRIPTIONS[props.state]}</p>
                  </div>
                </div>
                <ul class="security-modal-checks">
                  <For each={props.checks}>
                    {(c) => (
                      <li
                        class="security-modal-check"
                        data-ok={String(c.ok)}
                      >
                        <span class="security-modal-check-icon">
                          {c.ok ? <CheckIcon /> : <XIcon />}
                        </span>
                        <span class="security-modal-check-label">
                          {c.label}
                          <Show when={c.hint}>
                            <span class="security-modal-check-hint">
                              {c.hint}
                            </span>
                          </Show>
                        </span>
                      </li>
                    )}
                  </For>
                </ul>
              </div>
              <footer class="security-modal-footer">
                <button type="button" onClick={() => setOpen(false)}>
                  Close
                </button>
                <button
                  type="button"
                  class="security-modal-primary"
                  onClick={() => {
                    setOpen(false);
                    props.onOpenSettings();
                  }}
                >
                  Open Settings
                </button>
              </footer>
            </div>
          </div>
        </Portal>
      </Show>
    </>
  );
};

export default SecurityShield;
