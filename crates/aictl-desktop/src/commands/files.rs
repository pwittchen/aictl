//! Workspace file browser — directory listing + text read/write.
//!
//! Backs the right-side file pane: the webview asks for one directory
//! at a time (lazy expand) and round-trips text contents through
//! [`read_file`] / [`write_file`]. Every path is resolved against the
//! canonical workspace root so a hostile relative path with `..` cannot
//! escape, mirroring the same `canonicalize` + `starts_with` check
//! [`crate::commands::images::read_workspace_image`] uses.
//!
//! Binary files are rejected up front: the editor only deals in UTF-8
//! text, and surfacing raw bytes through the IPC channel would just
//! corrupt the document on round-trip. Detection is the same heuristic
//! `git` uses — first 8 KB scanned for NUL bytes.
//!
//! Reads cap at 2 MB so a stray multi-gigabyte file in the workspace
//! cannot freeze the renderer; writes have no explicit cap because the
//! editor itself is what produced the content.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::workspace;

const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

#[derive(Serialize)]
pub struct TreeEntry {
    pub name: String,
    /// Relative path inside the workspace (POSIX-style, slash separated).
    pub path: String,
    pub kind: EntryKind,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Dir,
    File,
}

#[derive(Serialize)]
pub struct FileContents {
    pub path: String,
    pub contents: String,
    pub size_bytes: u64,
}

/// List entries directly under `rel_dir` (relative to the workspace
/// root, or empty for the root itself). Hidden entries are returned —
/// the webview can decide whether to grey them out — but anything that
/// fails to stat is silently skipped rather than aborting the listing.
#[tauri::command]
pub async fn workspace_tree(rel_dir: String) -> Result<Vec<TreeEntry>, String> {
    let workspace = workspace::resolve()?.ok_or_else(|| "no workspace selected".to_string())?;
    let target = resolve_inside(&workspace, &rel_dir)?;
    let metadata = tokio::fs::metadata(&target)
        .await
        .map_err(|e| format!("failed to stat '{}': {e}", target.display()))?;
    if !metadata.is_dir() {
        return Err(format!("'{}' is not a directory", target.display()));
    }

    let mut entries: Vec<TreeEntry> = Vec::new();
    let mut reader = tokio::fs::read_dir(&target)
        .await
        .map_err(|e| format!("failed to read '{}': {e}", target.display()))?;
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|e| format!("failed to iterate '{}': {e}", target.display()))?
    {
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        let kind = if file_type.is_dir() {
            EntryKind::Dir
        } else if file_type.is_file() {
            EntryKind::File
        } else if file_type.is_symlink() {
            // Resolve the symlink one hop to classify it. Anything that
            // doesn't land on a regular file/dir inside the workspace is
            // dropped — a dangling symlink would just confuse the tree.
            let Ok(link_target) = tokio::fs::metadata(entry.path()).await else {
                continue;
            };
            if link_target.is_dir() {
                EntryKind::Dir
            } else if link_target.is_file() {
                EntryKind::File
            } else {
                continue;
            }
        } else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let mut rel = if rel_dir.trim().is_empty() {
            String::new()
        } else {
            normalize_rel(&rel_dir)
        };
        if !rel.is_empty() {
            rel.push('/');
        }
        rel.push_str(&name);
        entries.push(TreeEntry {
            name,
            path: rel,
            kind,
        });
    }
    entries.sort_by(|a, b| match (&a.kind, &b.kind) {
        (EntryKind::Dir, EntryKind::File) => std::cmp::Ordering::Less,
        (EntryKind::File, EntryKind::Dir) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(entries)
}

/// Read a UTF-8 text file under the workspace. Refuses binary files
/// (NUL byte detected in the first 8 KB) and anything bigger than
/// [`MAX_READ_BYTES`]. The webview surfaces both rejections as inline
/// notices instead of an editor.
#[tauri::command]
pub async fn workspace_read_file(rel_path: String) -> Result<FileContents, String> {
    let workspace = workspace::resolve()?.ok_or_else(|| "no workspace selected".to_string())?;
    let target = resolve_inside(&workspace, &rel_path)?;
    let metadata = tokio::fs::metadata(&target)
        .await
        .map_err(|e| format!("failed to stat '{}': {e}", target.display()))?;
    if !metadata.is_file() {
        return Err(format!("'{}' is not a regular file", target.display()));
    }
    if metadata.len() > MAX_READ_BYTES {
        return Err(format!(
            "file '{}' exceeds {} MB cap",
            target.display(),
            MAX_READ_BYTES / (1024 * 1024)
        ));
    }
    let bytes = tokio::fs::read(&target)
        .await
        .map_err(|e| format!("failed to read '{}': {e}", target.display()))?;
    if looks_binary(&bytes) {
        return Err(format!(
            "file '{}' looks binary — preview/edit is text-only",
            target.display()
        ));
    }
    let contents = String::from_utf8(bytes)
        .map_err(|_| format!("file '{}' is not valid UTF-8", target.display()))?;
    let size_bytes = metadata.len();
    Ok(FileContents {
        path: normalize_rel(&rel_path),
        contents,
        size_bytes,
    })
}

/// Create an empty text file inside the workspace. Refuses to overwrite
/// an existing entry; missing parent directories are created so a name
/// like `notes/today.md` works without two round-trips. Path validation
/// stays purely lexical (no `..`, no absolute roots) — same approach
/// the read/write/delete commands take, just without canonicalize since
/// the target doesn't exist yet.
#[tauri::command]
pub async fn workspace_create_file(rel_path: String) -> Result<(), String> {
    let workspace = workspace::resolve()?.ok_or_else(|| "no workspace selected".to_string())?;
    let cleaned = validate_new_rel(&rel_path)?;
    let target = workspace.join(&cleaned);
    if tokio::fs::try_exists(&target)
        .await
        .map_err(|e| format!("failed to stat '{}': {e}", target.display()))?
    {
        return Err(format!("'{cleaned}' already exists"));
    }
    if let Some(parent) = target.parent()
        && parent != workspace
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("failed to create parent of '{}': {e}", target.display()))?;
    }
    tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .await
        .map_err(|e| format!("failed to create '{}': {e}", target.display()))?;
    Ok(())
}

/// Create a directory inside the workspace. Refuses to overwrite an
/// existing entry; intermediate directories are auto-created.
#[tauri::command]
pub async fn workspace_create_dir(rel_path: String) -> Result<(), String> {
    let workspace = workspace::resolve()?.ok_or_else(|| "no workspace selected".to_string())?;
    let cleaned = validate_new_rel(&rel_path)?;
    let target = workspace.join(&cleaned);
    if tokio::fs::try_exists(&target)
        .await
        .map_err(|e| format!("failed to stat '{}': {e}", target.display()))?
    {
        return Err(format!("'{cleaned}' already exists"));
    }
    tokio::fs::create_dir_all(&target)
        .await
        .map_err(|e| format!("failed to create '{}': {e}", target.display()))?;
    Ok(())
}

/// Rename a file or directory inside the workspace. The new name is
/// taken as a single basename (no slashes), so the entry stays in its
/// current parent — moves across directories are intentionally not
/// supported here. Returns the workspace-relative path of the renamed
/// entry so the frontend can update its selection without a follow-up
/// listing.
#[tauri::command]
pub async fn workspace_rename(old_rel_path: String, new_name: String) -> Result<String, String> {
    let workspace = workspace::resolve()?.ok_or_else(|| "no workspace selected".to_string())?;
    let source = resolve_inside(&workspace, &old_rel_path)?;
    if source == workspace {
        return Err("refusing to rename the workspace root".to_string());
    }

    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return Err("name cannot be empty".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("name cannot contain slashes".to_string());
    }
    // Reject anything that isn't a single Normal component — guards against
    // `..`, drive letters, leading slashes, and stray dots.
    let mut comps = Path::new(trimmed).components();
    let only = comps.next();
    if comps.next().is_some() {
        return Err(format!("invalid name '{trimmed}'"));
    }
    if !matches!(only, Some(Component::Normal(_))) {
        return Err(format!("invalid name '{trimmed}'"));
    }

    let parent = source
        .parent()
        .ok_or_else(|| "no parent directory".to_string())?;
    let target = parent.join(trimmed);
    if target == source {
        // Renaming to the existing name is a noop — succeed without touching
        // the disk so the UI doesn't surface a spurious "already exists".
        return Ok(normalize_rel(&old_rel_path));
    }
    if tokio::fs::try_exists(&target)
        .await
        .map_err(|e| format!("failed to stat '{}': {e}", target.display()))?
    {
        return Err(format!("'{trimmed}' already exists"));
    }

    tokio::fs::rename(&source, &target)
        .await
        .map_err(|e| format!("failed to rename '{}': {e}", source.display()))?;

    let new_rel = target
        .strip_prefix(&workspace)
        .map_err(|e| format!("failed to compute new path: {e}"))?
        .to_string_lossy()
        .replace('\\', "/");
    Ok(new_rel)
}

/// Recursively delete a file or directory inside the workspace. The
/// frontend prompts the user for confirmation first; this command
/// performs no second-guessing beyond the path-jail check (a hostile
/// `..` couldn't reach a sibling of the workspace anyway). Deleting
/// the workspace root itself is rejected — clearing the workspace is
/// a separate operation that lives in Settings.
#[tauri::command]
pub async fn workspace_delete(rel_path: String) -> Result<(), String> {
    let workspace = workspace::resolve()?.ok_or_else(|| "no workspace selected".to_string())?;
    let target = resolve_inside(&workspace, &rel_path)?;
    if target == workspace {
        return Err("refusing to delete the workspace root".to_string());
    }
    let metadata = tokio::fs::symlink_metadata(&target)
        .await
        .map_err(|e| format!("failed to stat '{}': {e}", target.display()))?;
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        tokio::fs::remove_dir_all(&target)
            .await
            .map_err(|e| format!("failed to delete '{}': {e}", target.display()))?;
    } else {
        tokio::fs::remove_file(&target)
            .await
            .map_err(|e| format!("failed to delete '{}': {e}", target.display()))?;
    }
    Ok(())
}

/// Overwrite an existing text file inside the workspace. Refuses to
/// create new files (the file must already exist — keeps this command
/// minimally privileged) and refuses anything that resolves outside the
/// workspace root.
#[tauri::command]
pub async fn workspace_write_file(
    rel_path: String,
    contents: String,
) -> Result<FileContents, String> {
    let workspace = workspace::resolve()?.ok_or_else(|| "no workspace selected".to_string())?;
    let target = resolve_inside(&workspace, &rel_path)?;
    let metadata = tokio::fs::metadata(&target)
        .await
        .map_err(|e| format!("failed to stat '{}': {e}", target.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "'{}' is not a regular file — refusing to overwrite",
            target.display()
        ));
    }
    let bytes = contents.as_bytes();
    tokio::fs::write(&target, bytes)
        .await
        .map_err(|e| format!("failed to write '{}': {e}", target.display()))?;
    Ok(FileContents {
        path: normalize_rel(&rel_path),
        size_bytes: bytes.len() as u64,
        contents,
    })
}

fn resolve_inside(workspace: &Path, rel: &str) -> Result<PathBuf, String> {
    let trimmed = rel.trim().trim_start_matches('/');
    let candidate = if trimmed.is_empty() {
        workspace.to_path_buf()
    } else {
        // Reject any `..` component before touching the filesystem so a
        // hostile relative path can't reach for siblings of the
        // workspace via raw lexical traversal.
        let path = Path::new(trimmed);
        for c in path.components() {
            match c {
                Component::Normal(_) | Component::CurDir => {}
                _ => return Err(format!("invalid path '{trimmed}'")),
            }
        }
        workspace.join(path)
    };
    let canonical = candidate
        .canonicalize()
        .map_err(|e| format!("failed to resolve '{}': {e}", candidate.display()))?;
    if !canonical.starts_with(workspace) {
        return Err(format!(
            "refusing to access '{}' outside workspace",
            canonical.display()
        ));
    }
    Ok(canonical)
}

/// Lexical validation for a path that does not exist yet. Used by the
/// create commands — `resolve_inside` would call `canonicalize`, which
/// fails on a missing target. Same defence (`Component::Normal` only)
/// keeps a hostile name from reaching outside the workspace; the caller
/// then joins onto the workspace root and is guaranteed to land inside.
fn validate_new_rel(rel: &str) -> Result<String, String> {
    let trimmed = rel.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        return Err("name cannot be empty".to_string());
    }
    let path = Path::new(trimmed);
    for c in path.components() {
        match c {
            Component::Normal(_) => {}
            _ => return Err(format!("invalid name '{trimmed}'")),
        }
    }
    Ok(normalize_rel(trimmed))
}

fn normalize_rel(rel: &str) -> String {
    let trimmed = rel.trim().trim_start_matches('/').trim_end_matches('/');
    trimmed.replace('\\', "/")
}

fn looks_binary(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    head.contains(&0)
}
