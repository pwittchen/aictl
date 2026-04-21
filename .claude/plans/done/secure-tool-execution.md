# Plan: Secure Shell/Tool Execution in aictl

## Context

Currently, `aictl` has **zero** technical security controls on tool execution. The `exec_shell` tool passes commands directly to `sh -c`, file tools accept any path, and the only safeguard is user confirmation (bypassed entirely with `--auto`). This means a hallucinating or prompt-injected LLM could delete files, read credentials, or exfiltrate secrets.

## Approach: New `src/security.rs` Module + Integration

### 1. Security Policy Struct

Create `src/security.rs` with a `SecurityPolicy` loaded once at startup into a `static OnceLock` (matching existing config pattern):

```
SecurityPolicy
├── ShellPolicy { allowed_commands, blocked_commands, blocked_patterns }
├── PathPolicy { working_dir, restrict_to_cwd, blocked_paths, allowed_paths }
├── NetworkPolicy { allowed_domains, blocked_domains }
├── ResourcePolicy { shell_timeout_secs, max_file_write_bytes }
├── EnvPolicy { blocked_env_vars }
└── enabled: bool
```

### 2. Shell Command Validation (`check_shell`)

- **Block command substitution** by default: `` `...` ``, `$(...)`, `<(...)`, `>(...)`
- **Split compound commands** on `|`, `&&`, `||`, `;` — validate each segment
- **Extract base command**: strip env assignments (`FOO=bar cmd`), strip prefixes (`sudo`, `env`, `nohup`, `nice`, `time`, `command`, `builtin`), resolve full paths (`/usr/bin/rm` → `rm`), strip backslash escapes (`\rm` → `rm`)
- **Apply blacklist** (always wins): `rm`, `rmdir`, `mkfs`, `dd`, `shutdown`, `reboot`, `halt`, `poweroff`, `sudo`, `su`, `doas`, `eval`, `exec`, `nc`, `ncat`, `netcat`
- **Apply whitelist** (if configured): only listed commands are allowed

### 3. Path Validation (`check_path_read`, `check_path_write`, `check_dir`)

- Expand `~`, resolve relative paths against cwd
- **Canonicalize** via `std::fs::canonicalize()` (resolves symlinks — defeats symlink escape attacks)
- For writes to new files: canonicalize parent, then append filename
- **CWD jail** (default on): canonical path must be under working directory
- **Blocked paths** (defaults): `~/.ssh`, `~/.gnupg`, `~/.aictl`, `~/.aws`, `~/.config/gcloud`, `/etc/shadow`, `/etc/sudoers`
- Reject paths with null bytes
- Additional allowed paths configurable for outside-cwd access

### 4. Environment Variable Scrubbing

In `tool_exec_shell`, use `.env_clear()` on the `Command` and re-add only safe vars (`PATH`, `HOME`, `USER`, `TERM`, `LANG`, `SHELL`, `EDITOR`). Scrub anything matching `*_KEY`, `*_SECRET`, `*_TOKEN`, `*_PASSWORD` plus configurable blocklist. **This prevents API keys from leaking to shell subprocesses.**

### 5. Shell Timeout

Wrap `tokio::process::Command` with `tokio::time::timeout()` (default 30s). Prevents infinite loops from hanging.

### 6. Output Sanitization

Escape `<tool` and `</tool>` in tool result strings to prevent prompt injection via tool output (LLM response parser confusion).

### 7. Network Access Control

Optional domain allowlist/blocklist for `fetch_url` and `extract_website`. Default: unrestricted (needed for normal workflow).

### 8. Configuration (in `~/.aictl`)

```
AICTL_SECURITY=true                          # master switch
AICTL_SECURITY_CWD_RESTRICT=true             # cwd jail
AICTL_SECURITY_SHELL_ALLOWED=                # comma-separated whitelist (empty = all non-blocked)
AICTL_SECURITY_SHELL_BLOCKED=                # additional blocked commands
AICTL_SECURITY_BLOCKED_PATHS=                # additional blocked paths
AICTL_SECURITY_ALLOWED_PATHS=                # paths allowed outside cwd
AICTL_SECURITY_SHELL_TIMEOUT=30              # seconds
AICTL_SECURITY_MAX_WRITE=1048576             # bytes (1MB)
AICTL_SECURITY_BLOCK_SUBSHELL=true           # block $() and backticks
AICTL_SECURITY_BLOCKED_ENV=                  # additional env vars to scrub
```

### 9. Override Mechanism

- **`--unrestricted` CLI flag**: disables all checks, prints warning at startup
- **Per-setting config**: relax individual rules in `~/.aictl`
- **Soft denial**: blocked operations return error string as tool result (not hard crash), so LLM can adapt and explain to user

### 10. Integration Points

| File | Change |
|------|--------|
| `src/security.rs` | **New** — entire security module |
| `src/tools.rs` | Add `validate_tool()` gate at top of `execute_tool()`, env scrubbing + timeout in `tool_exec_shell` |
| `src/main.rs` | Add `mod security`, `--unrestricted` flag, `security::init()` call after config load |
| `src/commands.rs` | Add `/security` REPL command to show current policy |
| `src/config.rs` | Add security-related constants (defaults) |

### 11. Default Policy (works out of box, no config needed)

- Security: **on**
- CWD restriction: **on**
- Blocked commands: `rm`, `rmdir`, `sudo`, `su`, `doas`, `mkfs`, `dd`, `shutdown`, `reboot`, `halt`, `poweroff`, `eval`, `exec`, `nc`, `ncat`, `netcat`
- Command substitution: **blocked**
- Blocked paths: `~/.ssh`, `~/.gnupg`, `~/.aictl`, `~/.aws`, `~/.config/gcloud`, `/etc/shadow`, `/etc/sudoers`
- Shell timeout: **30s**
- Max write: **1MB**
- Env scrubbing: **on** (scrub `*_KEY`, `*_SECRET`, `*_TOKEN`, `*_PASSWORD`)

### 12. Additional Security Areas (future)

- **Write-then-execute detection**: track tool sequences, flag `write_file` → `exec_shell` on same path
- **Audit logging**: log all tool calls to `~/.aictl_audit.log`
- **Resource limits**: cap child process memory via ulimit in shell command prefix

## Verification

1. `cargo build` — compiles without errors
2. `cargo clippy` — no warnings
3. `cargo test` — all unit tests pass (especially security module tests)
4. Manual testing:
   - Run with `--auto` and verify blocked commands return denial messages
   - Verify path traversal (`../../etc/passwd`) is blocked
   - Verify `~/.ssh/id_rsa` read is blocked
   - Verify `--unrestricted` bypasses all checks
   - Verify env vars don't leak to shell subprocesses
   - Verify shell timeout kills long-running commands
