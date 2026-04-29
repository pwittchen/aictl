//! Remote catalogue for first-party skills.
//!
//! Browses and pulls skills from the project's GitHub repo under
//! `.aictl/skills/<name>/SKILL.md` at request time — the *list* is fetched
//! dynamically so new skills land in the catalogue the moment they merge to
//! master, no release required. The *source location* (owner / repo / branch)
//! is a compile-time constant here; unauthenticated public-repo reads
//! (~60/hr) are enough for browse-then-pull.
//!
//! This module is metadata-only: it enumerates skills, fetches their raw
//! SKILL.md bodies, and writes them to `~/.aictl/skills/<name>/SKILL.md`.
//! Mirrors [`crate::agents::remote`], differing only where skills'
//! one-directory-per-skill layout diverges from agents' flat `.md` files.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::config::http_client;
use crate::skills::{Parsed, is_valid_name, parse, skills_dir};

/// Upstream repo coordinates for the first-party skill catalogue. The list of
/// skills is dynamic (fetched on demand) but the source repo is pinned so
/// every build reaches the same catalogue regardless of release cadence.
pub const OWNER: &str = "pwittchen";
pub const REPO: &str = "aictl";
pub const BRANCH: &str = "master";
/// Subdirectory within the repo that holds the skill directories.
pub const REPO_PATH: &str = ".aictl/skills";

/// State of a catalogue entry relative to the local `~/.aictl/skills/` dir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Not yet pulled.
    NotPulled,
    /// Local file matches upstream byte-for-byte (or by blob-SHA).
    UpToDate,
    /// Local file exists but differs from upstream.
    UpstreamNewer,
}

/// One skill in the remote catalogue, already resolved to its raw body.
#[derive(Debug, Clone)]
pub struct RemoteSkill {
    /// Skill directory name — the string users type into `--pull-skill`.
    /// Always validated through [`crate::skills::is_valid_name`] before it
    /// enters a path on disk.
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    /// The raw SKILL.md (frontmatter + body) that a pull writes verbatim.
    pub body: String,
    /// GitHub blob SHA from the trees API. Currently unused — state is
    /// derived from a content comparison in [`local_state`] — but kept on
    /// the struct so future work can skip the SHA-256 hash when the blob
    /// sha is already authoritative.
    #[allow(dead_code)]
    pub blob_sha: String,
    /// Relation of this entry to what's currently on disk.
    pub state: State,
}

/// GitHub trees API response (we only care about a few fields).
#[derive(serde::Deserialize)]
struct TreeResponse {
    tree: Vec<TreeEntry>,
    #[serde(default)]
    truncated: bool,
}

#[derive(serde::Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
}

/// Fetch the catalogue listing and resolve every entry to a full
/// [`RemoteSkill`] (body + local state). Returns a user-facing error string
/// on any network, parse, or rate-limit failure so callers can render it
/// without exposing reqwest internals.
pub async fn list_skills() -> Result<Vec<RemoteSkill>, String> {
    let client = http_client();
    let url = format!("https://api.github.com/repos/{OWNER}/{REPO}/git/trees/{BRANCH}?recursive=1");
    let resp = client
        .get(&url)
        .header("User-Agent", format!("aictl/{}", crate::VERSION))
        .header("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("network error fetching catalogue: {e}"))?;

    if resp.status() == reqwest::StatusCode::FORBIDDEN
        || resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err(
            "GitHub rate limit reached (60/hr unauthenticated). Try again later.".to_string(),
        );
    }
    if !resp.status().is_success() {
        return Err(format!(
            "GitHub returned status {} for catalogue listing",
            resp.status()
        ));
    }

    let tree: TreeResponse = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse catalogue JSON: {e}"))?;
    if tree.truncated {
        return Err(
            "catalogue listing was truncated by GitHub — pull via --pull-skill".to_string(),
        );
    }

    // Accept only `.aictl/skills/<name>/SKILL.md` — one level of nesting, no
    // deeper. v1 pulls just SKILL.md; when bundled resources land the filter
    // will expand to include the whole subdirectory.
    let prefix = format!("{REPO_PATH}/");
    let candidates: Vec<(&TreeEntry, String)> = tree
        .tree
        .iter()
        .filter_map(|e| {
            if e.kind != "blob" || !e.path.starts_with(&prefix) {
                return None;
            }
            let suffix = &e.path[prefix.len()..];
            let (name, tail) = suffix.split_once('/')?;
            if tail != "SKILL.md" {
                return None;
            }
            Some((e, name.to_string()))
        })
        .collect();

    let mut out = Vec::with_capacity(candidates.len());
    for (entry, name) in candidates {
        if !is_valid_name(&name) {
            continue;
        }
        let body = fetch_raw(&name).await?;
        let Parsed {
            description,
            category,
            ..
        } = parse(&body);
        let state = local_state(&name, &body);
        out.push(RemoteSkill {
            name,
            description,
            category,
            body,
            blob_sha: entry.sha.clone(),
            state,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Fetch a single SKILL.md body by skill name from raw.githubusercontent.com.
/// Used both by the browse listing (for frontmatter) and by [`pull`] (for
/// writing to disk).
pub async fn fetch_raw(name: &str) -> Result<String, String> {
    if !is_valid_name(name) {
        return Err(format!("invalid skill name '{name}'"));
    }
    let url = format!(
        "https://raw.githubusercontent.com/{OWNER}/{REPO}/{BRANCH}/{REPO_PATH}/{name}/SKILL.md"
    );
    let client = http_client();
    let resp = client
        .get(&url)
        .header("User-Agent", format!("aictl/{}", crate::VERSION))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("network error fetching '{name}': {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!(
            "skill '{name}' not found in the official catalogue"
        ));
    }
    if !resp.status().is_success() {
        return Err(format!(
            "GitHub returned status {} for skill '{name}'",
            resp.status()
        ));
    }
    resp.text()
        .await
        .map_err(|e| format!("failed to read body for '{name}': {e}"))
}

/// Compare the on-disk copy of `name` (if any) to `upstream` by content.
/// Falls back to a byte-for-byte diff — cheap given how small these files
/// are — rather than tracking a separate hash next to the file.
pub fn local_state(name: &str, upstream: &str) -> State {
    let path = skills_dir().join(name).join("SKILL.md");
    let Ok(local) = std::fs::read_to_string(&path) else {
        return State::NotPulled;
    };
    if hash(&local) == hash(upstream) {
        State::UpToDate
    } else {
        State::UpstreamNewer
    }
}

fn hash(s: &str) -> [u8; 32] {
    Sha256::digest(s.as_bytes()).into()
}

/// Pull a single skill from the catalogue into
/// `~/.aictl/skills/<name>/SKILL.md`.
///
/// * `overwrite_decider` runs exactly when the target file already exists
///   and returns `true` to overwrite, `false` to abort. Use the `--force`
///   path by passing `|| true`.
///
/// Returns [`PullOutcome`] so the caller can render a different message for
/// a genuine install vs. a declined overwrite.
pub async fn pull<F>(name: &str, overwrite_decider: F) -> Result<PullOutcome, String>
where
    F: FnOnce() -> bool,
{
    if !is_valid_name(name) {
        return Err(format!(
            "invalid skill name '{name}' (use only letters, numbers, underscore, or dash)"
        ));
    }
    let body = fetch_raw(name).await?;
    let dir = skills_dir().join(name);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    let path = dir.join("SKILL.md");
    write_with_overwrite(&path, &body, overwrite_decider)
}

/// Write `body` to `path`, honouring an overwrite decision callback. Split
/// out so tests can exercise the decision logic without a mock HTTP layer.
pub fn write_with_overwrite<F>(
    path: &Path,
    body: &str,
    overwrite_decider: F,
) -> Result<PullOutcome, String>
where
    F: FnOnce() -> bool,
{
    if path.exists() {
        if !overwrite_decider() {
            return Ok(PullOutcome::SkippedExisting);
        }
        std::fs::write(path, body)
            .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
        Ok(PullOutcome::Overwritten)
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
        std::fs::write(path, body)
            .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
        Ok(PullOutcome::Installed)
    }
}

/// What [`pull`] did with the skill file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullOutcome {
    /// Fresh install — no previous file on disk.
    Installed,
    /// Existing file was overwritten.
    Overwritten,
    /// Existing file was kept; user declined the overwrite prompt.
    SkippedExisting,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_path(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = std::process::id();
        std::env::temp_dir().join(format!("aictl-skills-remote-test-{tag}-{pid}-{nanos}"))
    }

    #[test]
    fn write_fresh_install_creates_file() {
        let dir = unique_temp_path("install");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("skill-name").join("SKILL.md");
        let outcome = write_with_overwrite(&path, "body", || {
            panic!("should not ask about overwrite on fresh install")
        })
        .unwrap();
        assert_eq!(outcome, PullOutcome::Installed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "body");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_skips_when_overwrite_declined() {
        let dir = unique_temp_path("skip");
        let skill_dir = dir.join("skill-name");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        std::fs::write(&path, "original").unwrap();
        let outcome = write_with_overwrite(&path, "new body", || false).unwrap();
        assert_eq!(outcome, PullOutcome::SkippedExisting);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_overwrites_when_approved() {
        let dir = unique_temp_path("overwrite");
        let skill_dir = dir.join("skill-name");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        std::fs::write(&path, "original").unwrap();
        let outcome = write_with_overwrite(&path, "new body", || true).unwrap();
        assert_eq!(outcome, PullOutcome::Overwritten);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new body");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn local_state_not_pulled_when_file_missing() {
        // Use a path we know doesn't exist under skills_dir by picking a random name.
        let bogus_name = format!("aictl-definitely-not-real-{}", std::process::id());
        assert_eq!(local_state(&bogus_name, "anything"), State::NotPulled);
    }
}
