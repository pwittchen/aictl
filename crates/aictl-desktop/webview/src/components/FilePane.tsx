import type { Component } from "solid-js";
import { For, Show, createEffect, createSignal } from "solid-js";

import { ipc, type TreeEntry } from "../lib/ipc";
import CreatePrompt from "./CreatePrompt";

interface Props {
  /// Bumps when the workspace path changes — used to invalidate the
  /// expanded-folder state so a switch doesn't leave stale entries.
  workspaceKey: string;
  /// Bumps on every recursive-watcher pulse from the backend. Each
  /// increment triggers a re-fetch of every currently-expanded
  /// directory so newly-added or removed entries appear without a
  /// manual refresh.
  fsTick: number;
  onClose: () => void;
  onOpenFile: (path: string) => void;
}

const FilePane: Component<Props> = (props) => {
  const [expanded, setExpanded] = createSignal<Set<string>>(new Set([""]));
  const [children, setChildren] = createSignal<Record<string, TreeEntry[]>>(
    {},
  );
  const [loading, setLoading] = createSignal<Set<string>>(new Set());
  const [treeError, setTreeError] = createSignal<string | null>(null);
  // Path queued for delete-confirmation. Mirrors the Sidebar's session
  // delete pattern: first click on the trash icon arms the row, second
  // click confirms. Clicking another row's trash, or anywhere outside
  // the row, clears the arming.
  const [pendingDelete, setPendingDelete] = createSignal<string | null>(null);
  // Explicit user selection in the tree, distinct from the editor's
  // `openPath`. Tracks `kind` so the create-prompt can decide whether to
  // anchor the new entry on the selection itself (a dir) or on its
  // parent (a file). Click toggles: re-clicking the selected row clears
  // the selection, leaving the default action to fire on its own.
  const [selected, setSelected] = createSignal<
    { path: string; kind: "file" | "dir" } | null
  >(null);

  /// Workspace-relative directory the create-prompt should drop the new
  /// entry into. Empty string means the workspace root.
  const createBase = (): string => {
    const sel = selected();
    if (!sel) return "";
    if (sel.kind === "dir") return sel.path;
    const idx = sel.path.lastIndexOf("/");
    return idx === -1 ? "" : sel.path.slice(0, idx);
  };

  const toggleSelect = (path: string, kind: "file" | "dir") => {
    setSelected((cur) =>
      cur && cur.path === path && cur.kind === kind ? null : { path, kind },
    );
  };
  // Modal create-prompt: "file" or "dir" decides which command to call.
  // `error` surfaces backend rejections (already-exists, invalid name)
  // back into the modal so the user can fix the name without losing
  // their place. `null` mode hides the modal.
  const [createMode, setCreateMode] = createSignal<"file" | "dir" | null>(null);
  const [createError, setCreateError] = createSignal<string | null>(null);

  const beginCreate = (mode: "file" | "dir") => {
    setCreateMode(mode);
    setCreateError(null);
  };

  const cancelCreate = () => {
    setCreateMode(null);
    setCreateError(null);
  };

  const submitCreate = async (name: string) => {
    const mode = createMode();
    if (!mode) return;
    const base = createBase();
    const fullPath = base ? `${base}/${name}` : name;
    try {
      if (mode === "file") {
        await ipc.workspaceCreateFile(fullPath);
      } else {
        await ipc.workspaceCreateDir(fullPath);
      }
      cancelCreate();
      // The fs watcher will pulse and trigger a tree refresh shortly;
      // no need to refetch here.
    } catch (err) {
      setCreateError(`${err}`);
    }
  };

  const deleteEntry = async (rel: string) => {
    try {
      await ipc.workspaceDelete(rel);
      // The fs watcher will fire next and trigger a tree refresh; nothing
      // to do here beyond clearing the inline-confirm state.
    } catch (err) {
      setTreeError(`${err}`);
    } finally {
      setPendingDelete(null);
    }
  };

  const loadDir = async (rel: string) => {
    setLoading((s) => {
      const next = new Set(s);
      next.add(rel);
      return next;
    });
    try {
      const entries = await ipc.workspaceTree(rel);
      setChildren((c) => ({ ...c, [rel]: entries }));
      setTreeError(null);
    } catch (err) {
      setTreeError(`${err}`);
    } finally {
      setLoading((s) => {
        const next = new Set(s);
        next.delete(rel);
        return next;
      });
    }
  };

  const toggleDir = (rel: string) => {
    setExpanded((s) => {
      const next = new Set(s);
      if (next.has(rel)) {
        next.delete(rel);
      } else {
        next.add(rel);
        if (!(rel in children())) {
          void loadDir(rel);
        }
      }
      return next;
    });
  };

  // Initial load + workspace switch reset.
  createEffect(() => {
    void props.workspaceKey;
    setExpanded(new Set([""]));
    setChildren({});
    setPendingDelete(null);
    setSelected(null);
    setCreateMode(null);
    setCreateError(null);
    void loadDir("");
  });

  // Refresh on every fs-watcher pulse: re-fetch every directory we've
  // expanded so newly-added entries show up and removed ones disappear.
  // Skipping the very first tick avoids a redundant extra fetch right
  // after the workspace effect above seeds the root.
  let lastTick = -1;
  createEffect(() => {
    const tick = props.fsTick;
    if (lastTick === -1) {
      lastTick = tick;
      return;
    }
    if (tick === lastTick) return;
    lastTick = tick;
    for (const rel of expanded()) {
      void loadDir(rel);
    }
  });

  return (
    <aside class="file-pane" aria-label="Workspace files">
      <header class="file-pane-header">
        <span class="file-pane-title">Workspace Files</span>
        <span class="file-pane-header-actions">
          <button
            type="button"
            class="file-pane-action"
            aria-label="New file"
            title="New file"
            onClick={() => beginCreate("file")}
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              viewBox="0 0 16 16"
              fill="currentColor"
              width="14"
              height="14"
              aria-hidden="true"
            >
              <path d="M8.75 3.75a.75.75 0 0 0-1.5 0v3.5h-3.5a.75.75 0 0 0 0 1.5h3.5v3.5a.75.75 0 0 0 1.5 0v-3.5h3.5a.75.75 0 0 0 0-1.5h-3.5v-3.5Z" />
            </svg>
          </button>
          <button
            type="button"
            class="file-pane-action"
            aria-label="New directory"
            title="New directory"
            onClick={() => beginCreate("dir")}
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              viewBox="0 0 16 16"
              fill="currentColor"
              width="14"
              height="14"
              aria-hidden="true"
            >
              <path
                fill-rule="evenodd"
                clip-rule="evenodd"
                d="M3.5 2A1.5 1.5 0 0 0 2 3.5v9A1.5 1.5 0 0 0 3.5 14h9a1.5 1.5 0 0 0 1.5-1.5v-7A1.5 1.5 0 0 0 12.5 4H9.621a1.5 1.5 0 0 1-1.06-.44L7.439 2.44A1.5 1.5 0 0 0 6.38 2H3.5ZM8 6a.75.75 0 0 1 .75.75v1.5h1.5a.75.75 0 0 1 0 1.5h-1.5v1.5a.75.75 0 0 1-1.5 0v-1.5h-1.5a.75.75 0 0 1 0-1.5h1.5v-1.5A.75.75 0 0 1 8 6Z"
              />
            </svg>
          </button>
        </span>
        <button
          type="button"
          class="file-pane-close"
          aria-label="Close file pane"
          title="Close (⌘.)"
          onClick={props.onClose}
        >
          ✕
        </button>
      </header>
      <section class="file-pane-tree" aria-label="Workspace tree">
        <Show when={treeError()}>
          <div class="file-pane-error">{treeError()}</div>
        </Show>
        <TreeNode
          rel=""
          name="/"
          depth={0}
          isRoot
          expanded={expanded()}
          children_={children()}
          loading={loading()}
          selected={selected()}
          pendingDelete={pendingDelete()}
          onToggleDir={toggleDir}
          onOpenFile={props.onOpenFile}
          onToggleSelect={toggleSelect}
          onArmDelete={setPendingDelete}
          onConfirmDelete={(rel) => void deleteEntry(rel)}
        />
      </section>
      <Show when={createMode() !== null}>
        <CreatePrompt
          mode={createMode()!}
          base={createBase()}
          error={createError()}
          onSubmit={(name) => void submitCreate(name)}
          onCancel={cancelCreate}
        />
      </Show>
    </aside>
  );
};

interface TreeNodeProps {
  rel: string;
  name: string;
  depth: number;
  isRoot?: boolean;
  expanded: Set<string>;
  /// Renamed to dodge SolidJS's reserved `children` prop name.
  children_: Record<string, TreeEntry[]>;
  loading: Set<string>;
  /// Currently-selected entry, drives the row highlight. Also tells the
  /// create-prompt where to land new entries.
  selected: { path: string; kind: "file" | "dir" } | null;
  /// Path that's currently armed for deletion (first trash click). The
  /// matching row swaps its trash icon for a "delete?" confirm button.
  pendingDelete: string | null;
  onToggleDir: (rel: string) => void;
  onOpenFile: (rel: string) => void;
  onToggleSelect: (rel: string, kind: "file" | "dir") => void;
  onArmDelete: (rel: string | null) => void;
  onConfirmDelete: (rel: string) => void;
}

const TreeNode: Component<TreeNodeProps> = (props) => {
  const isOpen = () => props.expanded.has(props.rel);
  const entries = () => props.children_[props.rel] ?? null;
  const isLoading = () => props.loading.has(props.rel);

  const isSelected = () =>
    props.selected !== null &&
    props.selected.path === props.rel &&
    props.selected.kind === "dir";

  return (
    <div class="tree-node">
      <Show when={!props.isRoot}>
        <div
          class="tree-row"
          data-selected={String(isSelected())}
          style={{ "padding-left": `${props.depth * 12 + 6}px` }}
        >
          <button
            type="button"
            class="tree-row-main"
            onClick={() => {
              props.onToggleSelect(props.rel, "dir");
              props.onToggleDir(props.rel);
            }}
          >
            <span class="tree-caret" aria-hidden="true">
              {isOpen() ? "▾" : "▸"}
            </span>
            <span class="tree-icon tree-icon-folder" aria-hidden="true">
              <FolderIcon open={isOpen()} />
            </span>
            <span class="tree-label">{props.name}</span>
          </button>
          <TrashAction
            rel={props.rel}
            label={`directory ${props.name}`}
            armed={props.pendingDelete === props.rel}
            onArm={props.onArmDelete}
            onConfirm={props.onConfirmDelete}
          />
        </div>
      </Show>
      <Show when={isOpen()}>
        <Show when={isLoading() && !entries()}>
          <div
            class="tree-empty"
            style={{ "padding-left": `${(props.depth + 1) * 12 + 6}px` }}
          >
            loading…
          </div>
        </Show>
        <Show when={entries() !== null && entries()!.length === 0}>
          <div
            class="tree-empty"
            style={{ "padding-left": `${(props.depth + 1) * 12 + 6}px` }}
          >
            empty
          </div>
        </Show>
        <For each={entries() ?? []}>
          {(entry) =>
            entry.kind === "dir" ? (
              <TreeNode
                rel={entry.path}
                name={entry.name}
                depth={props.depth + 1}
                expanded={props.expanded}
                children_={props.children_}
                loading={props.loading}
                selected={props.selected}
                pendingDelete={props.pendingDelete}
                onToggleDir={props.onToggleDir}
                onOpenFile={props.onOpenFile}
                onToggleSelect={props.onToggleSelect}
                onArmDelete={props.onArmDelete}
                onConfirmDelete={props.onConfirmDelete}
              />
            ) : (
              <div
                class="tree-row tree-row-file"
                data-selected={String(
                  props.selected !== null &&
                    props.selected.path === entry.path &&
                    props.selected.kind === "file",
                )}
                style={{
                  "padding-left": `${(props.depth + 1) * 12 + 6}px`,
                }}
              >
                <button
                  type="button"
                  class="tree-row-main"
                  onClick={() => {
                    props.onToggleSelect(entry.path, "file");
                    props.onOpenFile(entry.path);
                  }}
                  title={entry.path}
                >
                  <span class="tree-caret" aria-hidden="true" />
                  <span class="tree-icon" aria-hidden="true">
                    ·
                  </span>
                  <span class="tree-label">{entry.name}</span>
                </button>
                <TrashAction
                  rel={entry.path}
                  label={`file ${entry.name}`}
                  armed={props.pendingDelete === entry.path}
                  onArm={props.onArmDelete}
                  onConfirm={props.onConfirmDelete}
                />
              </div>
            )
          }
        </For>
      </Show>
    </div>
  );
};

/// Folder glyphs — the same pair the titlebar's file-pane toggle uses,
/// so an expanded directory in the tree visually echoes the open-folder
/// icon up top and a collapsed one mirrors the closed-folder state.
/// Wrapped in `<Show>` because a bare ternary inside a SolidJS component
/// body is evaluated once at creation and would never re-render when
/// the `open` prop flipped.
const FolderIcon: Component<{ open: boolean }> = (props) => (
  <Show
    when={props.open}
    fallback={
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="0 0 16 16"
        fill="currentColor"
        width="13"
        height="13"
        aria-hidden="true"
      >
        <path d="M2 3.5A1.5 1.5 0 0 1 3.5 2h2.879a1.5 1.5 0 0 1 1.06.44l1.122 1.12A1.5 1.5 0 0 0 9.62 4H12.5A1.5 1.5 0 0 1 14 5.5v1.401a2.986 2.986 0 0 0-1.5-.401h-9c-.546 0-1.059.146-1.5.401V3.5ZM2 9.5v3A1.5 1.5 0 0 0 3.5 14h9a1.5 1.5 0 0 0 1.5-1.5v-3A1.5 1.5 0 0 0 12.5 8h-9A1.5 1.5 0 0 0 2 9.5Z" />
      </svg>
    }
  >
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 16 16"
      fill="currentColor"
      width="13"
      height="13"
      aria-hidden="true"
    >
      <path d="M3 3.5A1.5 1.5 0 0 1 4.5 2h1.879a1.5 1.5 0 0 1 1.06.44l1.122 1.12A1.5 1.5 0 0 0 9.62 4H11.5A1.5 1.5 0 0 1 13 5.5v1H3v-3ZM3.081 8a1.5 1.5 0 0 0-1.423 1.974l1 3A1.5 1.5 0 0 0 4.081 14h7.838a1.5 1.5 0 0 0 1.423-1.026l1-3A1.5 1.5 0 0 0 12.919 8H3.081Z" />
    </svg>
  </Show>
);

interface TrashProps {
  rel: string;
  /// Used as the aria-label for screen readers ("delete file foo.txt").
  label: string;
  armed: boolean;
  onArm: (rel: string | null) => void;
  onConfirm: (rel: string) => void;
}

const TrashAction: Component<TrashProps> = (props) => (
  <Show
    when={props.armed}
    fallback={
      <button
        type="button"
        class="tree-row-trash"
        aria-label={`Delete ${props.label}`}
        title="Delete"
        onClick={(e) => {
          e.stopPropagation();
          props.onArm(props.rel);
        }}
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          viewBox="0 0 16 16"
          fill="currentColor"
          width="14"
          height="14"
          aria-hidden="true"
        >
          <path
            fill-rule="evenodd"
            clip-rule="evenodd"
            d="M5 3.25V4H2.75a.75.75 0 0 0 0 1.5h.3l.815 8.15A1.5 1.5 0 0 0 5.357 15h5.285a1.5 1.5 0 0 0 1.493-1.35l.815-8.15h.3a.75.75 0 0 0 0-1.5H11v-.75A2.25 2.25 0 0 0 8.75 1h-1.5A2.25 2.25 0 0 0 5 3.25Zm2.25-.75a.75.75 0 0 0-.75.75V4h3v-.75a.75.75 0 0 0-.75-.75h-1.5ZM6.05 6a.75.75 0 0 1 .787.713l.275 5.5a.75.75 0 0 1-1.498.075l-.275-5.5A.75.75 0 0 1 6.05 6Zm3.9 0a.75.75 0 0 1 .712.787l-.275 5.5a.75.75 0 0 1-1.498-.075l.275-5.5a.75.75 0 0 1 .786-.711Z"
          />
        </svg>
      </button>
    }
  >
    <span class="tree-row-confirm">
      <button
        type="button"
        class="tree-row-confirm-yes"
        title={`Confirm delete ${props.label}`}
        onClick={(e) => {
          e.stopPropagation();
          props.onConfirm(props.rel);
        }}
      >
        Delete?
      </button>
      <button
        type="button"
        class="tree-row-confirm-no"
        aria-label="Cancel delete"
        title="Cancel"
        onClick={(e) => {
          e.stopPropagation();
          props.onArm(null);
        }}
      >
        ✕
      </button>
    </span>
  </Show>
);

export default FilePane;
