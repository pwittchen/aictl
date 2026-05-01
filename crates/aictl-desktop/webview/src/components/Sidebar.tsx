import type { Component } from "solid-js";
import { For, createSignal } from "solid-js";

// Sidebar entries. Phase 2 only renders the chat tab as active —
// agents/skills/sessions/settings tabs are stubs that the later phases
// fill in. Keeping them in the layout from day one means the visual
// rhythm doesn't shift when those features land.
const TABS = [
  "Chat",
  "Sessions",
  "Agents",
  "Skills",
  "Tools",
  "Stats",
  "Settings",
] as const;
type Tab = (typeof TABS)[number];

const Sidebar: Component = () => {
  const [active, setActive] = createSignal<Tab>("Chat");
  return (
    <aside class="sidebar">
      <For each={TABS}>
        {(tab) => (
          <button
            type="button"
            data-active={String(active() === tab)}
            onClick={() => setActive(tab)}
          >
            {tab}
          </button>
        )}
      </For>
    </aside>
  );
};

export default Sidebar;
