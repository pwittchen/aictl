import type { Component } from "solid-js";
import {
  For,
  Show,
  createEffect,
  createResource,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";

import { ipc, type ActiveSession, type SessionRow } from "../lib/ipc";
import ConfirmDelete from "./ConfirmDelete";

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

  // Tracks whether the rename was cancelled via Esc, so the input's
  // blur handler doesn't race-submit the half-typed name right after
  // we've already set `renamingId` to null. Cleared every time a fresh
  // rename begins.
  let renameCancelled = false;

  const beginRename = (row: SessionRow) => {
    renameCancelled = false;
    setRenamingId(row.id);
    setRenameValue(row.name ?? "");
  };

  const cancelRename = () => {
    renameCancelled = true;
    setRenamingId(null);
  };

  const submitRename = async () => {
    if (renameCancelled) {
      renameCancelled = false;
      return;
    }
    const id = renamingId();
    const name = renameValue().trim();
    if (!id || !name) {
      setRenamingId(null);
      return;
    }
    await props.onRenameSession(id, name);
    setRenamingId(null);
  };

  // Window-level Esc handler so the user can cancel an in-flight rename
  // even after clicking somewhere outside the input — without this they
  // would be stranded with no obvious exit when the input lost focus
  // and the submit was a no-op (empty name).
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (renamingId() === null) return;
      e.preventDefault();
      cancelRename();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

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
                            cancelRename();
                            // Drop focus so the trailing blur fires
                            // immediately, sees the cancelled flag, and
                            // exits without committing the half-typed
                            // name.
                            (e.currentTarget as HTMLInputElement).blur();
                          }
                        }}
                        onBlur={() => void submitRename()}
                      />
                    </Show>
                    <span class="session-when">
                      {fmtRelative(row.modified_secs)}
                    </span>
                  </div>
                  <Show when={renamingId() !== row.id}>
                    <div class="session-row-actions">
                      <button
                        type="button"
                        class="ghost mini session-row-edit"
                        aria-label="Rename session"
                        title="Rename"
                        onClick={(e) => {
                          e.stopPropagation();
                          beginRename(row);
                        }}
                      >
                        <svg
                          xmlns="http://www.w3.org/2000/svg"
                          viewBox="0 0 16 16"
                          fill="currentColor"
                          width="12"
                          height="12"
                          aria-hidden="true"
                        >
                          <path
                            fill-rule="evenodd"
                            clip-rule="evenodd"
                            d="M11.013 2.513a1.75 1.75 0 0 1 2.475 2.474L6.226 12.25a2.751 2.751 0 0 1-.892.596l-2.047.848a.75.75 0 0 1-.98-.98l.848-2.047a2.75 2.75 0 0 1 .596-.892l7.262-7.261Z"
                          />
                        </svg>
                      </button>
                      <button
                        type="button"
                        class="ghost mini session-row-trash"
                        aria-label="Delete session"
                        title="Delete session"
                        onClick={(e) => {
                          e.stopPropagation();
                          setPendingDelete(row.id);
                        }}
                      >
                        <svg
                          xmlns="http://www.w3.org/2000/svg"
                          viewBox="0 0 16 16"
                          fill="currentColor"
                          width="12"
                          height="12"
                          aria-hidden="true"
                        >
                          <path
                            fill-rule="evenodd"
                            clip-rule="evenodd"
                            d="M5 3.25V4H2.75a.75.75 0 0 0 0 1.5h.3l.815 8.15A1.5 1.5 0 0 0 5.357 15h5.285a1.5 1.5 0 0 0 1.493-1.35l.815-8.15h.3a.75.75 0 0 0 0-1.5H11v-.75A2.25 2.25 0 0 0 8.75 1h-1.5A2.25 2.25 0 0 0 5 3.25Zm2.25-.75a.75.75 0 0 0-.75.75V4h3v-.75a.75.75 0 0 0-.75-.75h-1.5ZM6.05 6a.75.75 0 0 1 .787.713l.275 5.5a.75.75 0 0 1-1.498.075l-.275-5.5A.75.75 0 0 1 6.05 6Zm3.9 0a.75.75 0 0 1 .712.787l-.275 5.5a.75.75 0 0 1-1.498-.075l.275-5.5a.75.75 0 0 1 .786-.711Z"
                          />
                        </svg>
                      </button>
                    </div>
                  </Show>
                </li>
              );
            }}
          </For>
          <Show when={(sessions() ?? []).length === 0}>
            <li class="session-empty">No sessions yet — start chatting.</li>
          </Show>
        </ul>

        <Show when={(sessions() ?? []).length > 0}>
          <button
            type="button"
            class="ghost danger-link"
            onClick={() => setShowClearAll(true)}
          >
            Clear all sessions…
          </button>
        </Show>
      </div>

      <Show when={showClearAll()}>
        {(() => {
          const count = (sessions() ?? []).length;
          return (
            <ConfirmDelete
              title="Clear all sessions"
              detail={`${count} session${count === 1 ? "" : "s"}`}
              note="Every session and its transcript will be removed."
              onCancel={() => setShowClearAll(false)}
              onConfirm={() => {
                setShowClearAll(false);
                void props.onClearAll();
              }}
            />
          );
        })()}
      </Show>

      <Show when={pendingDelete()}>
        {(id) => {
          const row = () =>
            (sessions() ?? []).find((s) => s.id === id());
          const label = () => {
            const r = row();
            if (!r) return id();
            return r.name ?? `(${shortId(r.id)})`;
          };
          return (
            <ConfirmDelete
              title="Delete session"
              detail={label()}
              note="The session and its transcript will be removed."
              onCancel={() => setPendingDelete(null)}
              onConfirm={() => {
                const target = id();
                setPendingDelete(null);
                void props.onDeleteSession(target);
              }}
            />
          );
        }}
      </Show>
    </aside>
  );
};

export default Sidebar;
