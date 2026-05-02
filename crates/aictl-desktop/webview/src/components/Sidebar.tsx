import type { Component } from "solid-js";
import { For, Show, createEffect, createResource, createSignal } from "solid-js";

import { ipc, type ActiveSession, type SessionRow } from "../lib/ipc";

interface Props {
  activeSession: ActiveSession;
  /// Bumped by App.tsx whenever the transcript changes (turn ended,
  /// session deleted, etc.) so the resource refetches.
  refreshKey: number;
  onSelectSession: (id: string) => void | Promise<void>;
  onNewSession: () => void | Promise<void>;
  onNewIncognito: () => void | Promise<void>;
  onDeleteSession: (id: string) => void | Promise<void>;
  onClearAll: () => void | Promise<void>;
  onRenameSession: (id: string, name: string) => void | Promise<void>;
}

const fmtRelative = (secs: number): string => {
  if (!secs) return "";
  const ageSec = Math.max(0, Math.floor(Date.now() / 1000) - secs);
  if (ageSec < 60) return "just now";
  if (ageSec < 3600) return `${Math.floor(ageSec / 60)}m ago`;
  if (ageSec < 86400) return `${Math.floor(ageSec / 3600)}h ago`;
  if (ageSec < 86400 * 30) return `${Math.floor(ageSec / 86400)}d ago`;
  const dt = new Date(secs * 1000);
  return dt.toLocaleDateString();
};

const shortId = (id: string): string => id.slice(0, 8);

const Sidebar: Component<Props> = (props) => {
  const [sessions, { refetch }] = createResource(
    () => props.refreshKey,
    () => ipc.listSessions(),
  );
  const [filter, setFilter] = createSignal("");
  const [renamingId, setRenamingId] = createSignal<string | null>(null);
  const [renameValue, setRenameValue] = createSignal("");
  const [pendingDelete, setPendingDelete] = createSignal<string | null>(null);
  const [showClearAll, setShowClearAll] = createSignal(false);

  // The resource is keyed on `refreshKey`, but Solid only schedules a
  // fetch when the key actually changes. App.tsx may bump it to the same
  // value the resource already saw (e.g. after a turn that didn't
  // mutate the on-disk size yet) — refetch defensively after every prop
  // change to keep mtimes fresh.
  createEffect(() => {
    void props.refreshKey;
    refetch();
  });

  const filtered = (): SessionRow[] => {
    const q = filter().toLowerCase().trim();
    const all = sessions() ?? [];
    if (!q) return all;
    return all.filter(
      (s) =>
        s.id.toLowerCase().includes(q) ||
        (s.name?.toLowerCase().includes(q) ?? false),
    );
  };

  const beginRename = (row: SessionRow) => {
    setRenamingId(row.id);
    setRenameValue(row.name ?? "");
  };

  const submitRename = async () => {
    const id = renamingId();
    const name = renameValue().trim();
    if (!id || !name) {
      setRenamingId(null);
      return;
    }
    await props.onRenameSession(id, name);
    setRenamingId(null);
  };

  return (
    <aside class="sidebar">
      <div class="sidebar-section sessions-section">
        <div class="sidebar-header">
          <span>Sessions</span>
          <div class="sidebar-actions">
            <button
              type="button"
              class="ghost"
              title="New session"
              onClick={() => void props.onNewSession()}
            >
              + New
            </button>
            <button
              type="button"
              class="ghost"
              title="Start an incognito session (not persisted)"
              onClick={() => void props.onNewIncognito()}
            >
              Incognito
            </button>
          </div>
        </div>

        <input
          class="sidebar-filter"
          type="text"
          placeholder="filter sessions…"
          value={filter()}
          onInput={(e) => setFilter(e.currentTarget.value)}
        />

        <Show when={props.activeSession.incognito}>
          <div class="sidebar-banner">
            Incognito — this turn is <em>not</em> being persisted.
          </div>
        </Show>

        <ul class="session-list">
          <For each={filtered()}>
            {(row) => {
              const isActive = () =>
                row.id === props.activeSession.id || row.active;
              return (
                <li
                  class="session-row"
                  data-active={String(isActive())}
                  onClick={() => {
                    if (renamingId() === row.id) return;
                    void props.onSelectSession(row.id);
                  }}
                >
                  <div class="session-meta">
                    <Show
                      when={renamingId() === row.id}
                      fallback={
                        <span class="session-name">
                          {row.name ?? `(${shortId(row.id)})`}
                        </span>
                      }
                    >
                      <input
                        class="session-rename"
                        type="text"
                        value={renameValue()}
                        autofocus
                        onClick={(e) => e.stopPropagation()}
                        onInput={(e) => setRenameValue(e.currentTarget.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            e.preventDefault();
                            void submitRename();
                          } else if (e.key === "Escape") {
                            e.preventDefault();
                            setRenamingId(null);
                          }
                        }}
                        onBlur={() => void submitRename()}
                      />
                    </Show>
                    <span class="session-when">
                      {fmtRelative(row.modified_secs)}
                    </span>
                  </div>
                  <div class="session-row-actions">
                    <button
                      type="button"
                      class="ghost mini"
                      title="Rename"
                      onClick={(e) => {
                        e.stopPropagation();
                        beginRename(row);
                      }}
                    >
                      ✎
                    </button>
                    <Show
                      when={pendingDelete() === row.id}
                      fallback={
                        <button
                          type="button"
                          class="ghost mini"
                          title="Delete session"
                          onClick={(e) => {
                            e.stopPropagation();
                            setPendingDelete(row.id);
                          }}
                        >
                          ✕
                        </button>
                      }
                    >
                      <button
                        type="button"
                        class="ghost mini danger"
                        title="Confirm delete"
                        onClick={(e) => {
                          e.stopPropagation();
                          setPendingDelete(null);
                          void props.onDeleteSession(row.id);
                        }}
                      >
                        Delete?
                      </button>
                    </Show>
                  </div>
                </li>
              );
            }}
          </For>
          <Show when={(sessions() ?? []).length === 0}>
            <li class="session-empty">No sessions yet — start chatting.</li>
          </Show>
        </ul>

        <Show when={(sessions() ?? []).length > 0}>
          <Show
            when={showClearAll()}
            fallback={
              <button
                type="button"
                class="ghost danger-link"
                onClick={() => setShowClearAll(true)}
              >
                Clear all sessions…
              </button>
            }
          >
            <div class="confirm-row">
              <span>Delete every session?</span>
              <button
                type="button"
                class="ghost mini"
                onClick={() => setShowClearAll(false)}
              >
                Cancel
              </button>
              <button
                type="button"
                class="ghost mini danger"
                onClick={() => {
                  setShowClearAll(false);
                  void props.onClearAll();
                }}
              >
                Yes
              </button>
            </div>
          </Show>
        </Show>
      </div>

      {/*
        Sidebar footer — non-session menu items live here so they stay
        pinned at the bottom while the session list scrolls. Settings is
        the only entry for now; Agents / Skills / Stats / Help land in
        later phases of the desktop plan.
      */}
      <nav class="sidebar-section bottom-section">
        <button
          type="button"
          class="bottom-item"
          onClick={() => {
            // Settings UI lands in Phase 5 of the desktop plan. Keeping
            // the button visible now so the layout doesn't shift when
            // the pane is implemented.
            console.info("settings UI is not yet implemented");
          }}
        >
          Settings
        </button>
      </nav>
    </aside>
  );
};

export default Sidebar;
