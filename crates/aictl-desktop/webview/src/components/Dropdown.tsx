import type { Component, JSX } from "solid-js";
import { For, Show, createMemo, createSignal, onCleanup } from "solid-js";

export interface DropdownOption {
  value: string;
  label: string;
}

interface Props {
  options: DropdownOption[];
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  id?: string;
  ariaLabel?: string;
  placeholder?: string;
  class?: string;
  style?: JSX.CSSProperties;
}

export const Dropdown: Component<Props> = (props) => {
  const [open, setOpen] = createSignal(false);
  let buttonRef: HTMLButtonElement | undefined;
  let menuRef: HTMLDivElement | undefined;

  const selected = createMemo(() =>
    props.options.find((o) => o.value === props.value),
  );

  const close = () => setOpen(false);
  const toggle = () => {
    if (props.disabled) return;
    setOpen((v) => !v);
  };
  const choose = (v: string) => {
    close();
    if (v !== props.value) props.onChange(v);
    queueMicrotask(() => buttonRef?.focus());
  };

  const onDocPointer = (e: MouseEvent) => {
    if (!open()) return;
    const t = e.target;
    if (!(t instanceof Node)) return;
    if (buttonRef?.contains(t) || menuRef?.contains(t)) return;
    close();
  };
  const onDocKey = (e: KeyboardEvent) => {
    if (!open()) return;
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      buttonRef?.focus();
    }
  };
  document.addEventListener("mousedown", onDocPointer);
  document.addEventListener("keydown", onDocKey);
  onCleanup(() => {
    document.removeEventListener("mousedown", onDocPointer);
    document.removeEventListener("keydown", onDocKey);
  });

  return (
    <span class="dropdown-anchor" classList={{ [props.class ?? ""]: !!props.class }}>
      <button
        type="button"
        class="dropdown"
        id={props.id}
        ref={(el) => (buttonRef = el)}
        disabled={props.disabled}
        aria-haspopup="listbox"
        aria-expanded={open() ? "true" : "false"}
        aria-label={props.ariaLabel}
        style={props.style}
        onClick={toggle}
      >
        <span class="dropdown-label">
          {selected()?.label ?? props.placeholder ?? props.value}
        </span>
      </button>
      <Show when={open()}>
        <div
          class="dropdown-menu"
          role="listbox"
          ref={(el) => (menuRef = el)}
        >
          <ul class="dropdown-list">
            <For each={props.options}>
              {(opt) => {
                const active = () => opt.value === props.value;
                return (
                  <li>
                    <button
                      type="button"
                      class="dropdown-item"
                      role="option"
                      data-active={String(active())}
                      aria-selected={active() ? "true" : "false"}
                      onClick={() => choose(opt.value)}
                    >
                      <span class="dropdown-item-label">{opt.label}</span>
                    </button>
                  </li>
                );
              }}
            </For>
          </ul>
        </div>
      </Show>
    </span>
  );
};
