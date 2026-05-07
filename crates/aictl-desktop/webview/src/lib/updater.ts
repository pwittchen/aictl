// Thin wrapper over `@tauri-apps/plugin-updater` and `plugin-process` so
// the rest of the webview only ever imports `runUpdate` / `checkUpdate`.
//
// The plugin's own JS API exposes `check()` (returns `Update | null`)
// and `update.downloadAndInstall(onEvent)` which streams typed events
// (`Started { contentLength }`, `Progress { chunkLength }`, `Finished`).
// We re-shape that into a single byte-counter so the UI can render a
// determinate bar without re-reading the plugin's event union shape.
//
// `relaunch()` lives in `plugin-process` in Tauri v2 — it's the call
// that hands control to the freshly-installed bundle. We only invoke
// it after `Finished`; on macOS the bundle has been replaced in place
// at that point and the OS launches the new copy.

import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  notes: string | null;
  date: string | null;
}

export interface UpdateProgress {
  /// `null` until the download header arrives — we render an
  /// indeterminate bar in that window.
  totalBytes: number | null;
  downloadedBytes: number;
  /// Set to `installing` between `Finished` and `relaunch`. Brief on
  /// macOS (untar + atomic rename) but visible enough that the user
  /// shouldn't think the app froze.
  phase: "checking" | "downloading" | "installing" | "restarting" | "done" | "error";
  error?: string;
}

/// Probe the manifest endpoint configured in `tauri.conf.json`. Returns
/// `null` when the running build is already the latest version (the
/// plugin's `check()` returns `null` in that case) or when the manifest
/// can't be reached / parsed — we surface fetch failures as throws so
/// the caller can distinguish "no update" from "update check broken".
export async function checkUpdate(): Promise<UpdateInfo | null> {
  const update: Update | null = await check();
  if (!update) return null;
  return {
    version: update.version,
    currentVersion: update.currentVersion,
    notes: update.body ?? null,
    date: update.date ?? null,
  };
}

/// Drive the full download → install → relaunch flow, calling
/// `onProgress` synchronously on every plugin event so the caller can
/// render a live progress bar. The promise resolves only if the
/// relaunch fails — on success the new process replaces this one.
export async function runUpdate(
  onProgress: (p: UpdateProgress) => void,
): Promise<void> {
  onProgress({ phase: "checking", totalBytes: null, downloadedBytes: 0 });
  const update = await check();
  if (!update) {
    onProgress({
      phase: "done",
      totalBytes: 0,
      downloadedBytes: 0,
    });
    return;
  }

  let totalBytes: number | null = null;
  let downloadedBytes = 0;

  await update.downloadAndInstall((event) => {
    switch (event.event) {
      case "Started": {
        totalBytes =
          typeof event.data.contentLength === "number"
            ? event.data.contentLength
            : null;
        downloadedBytes = 0;
        onProgress({ phase: "downloading", totalBytes, downloadedBytes });
        break;
      }
      case "Progress": {
        downloadedBytes += event.data.chunkLength;
        onProgress({ phase: "downloading", totalBytes, downloadedBytes });
        break;
      }
      case "Finished": {
        onProgress({ phase: "installing", totalBytes, downloadedBytes });
        break;
      }
    }
  });

  onProgress({ phase: "restarting", totalBytes, downloadedBytes });
  await relaunch();
}
