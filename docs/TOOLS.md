# Tools & security

aictl runs an agent loop: the LLM can invoke tools, see their results, and continue reasoning until it produces a final answer.

By default, every tool call requires confirmation (y/N prompt). Use `--auto` to skip confirmation and run autonomously.

## Built-in tools

| Tool | Description |
|------|-------------|
| `exec_shell` | Execute a shell command via `sh -c` |
| `read_file` | Read the contents of a file |
| `write_file` | Write content to a file (first line = path, rest = content) |
| `remove_file` | Remove (delete) a file (regular files only, not directories) |
| `create_directory` | Create a directory and any missing parent directories |
| `list_directory` | List files and directories at a path with `[FILE]`/`[DIR]`/`[LINK]` prefixes |
| `search_files` | Search file contents by pattern (grep regex) with optional directory scope |
| `edit_file` | Apply a targeted find-and-replace edit to a file (exact unique match required) |
| `diff_files` | Compare two text files and return a unified diff with 3 lines of context. First line is the "before" path, second line is the "after" path. Works in-process via an LCS DP table — no external `diff` binary, no platform drift. Refuses to diff files longer than 2000 lines each |
| `search_web_fc` | Primary web search via Firecrawl API (requires `FIRECRAWL_API_KEY`). Returns titles, URLs, and descriptions of matching results |
| `search_web_ddg` | Fallback web search via DuckDuckGo Instant Answer API (no API key). Same `[N] title / URL / description` output shape as `search_web_fc`. The agent picks this automatically when `search_web_fc` is disabled or errors out, and can be selected explicitly by saying "use duckduckgo" / "use duck duck go" in the prompt |
| `find_files` | Find files matching a glob pattern (e.g. `**/*.rs`) with optional base directory |
| `fetch_url` | Fetch a URL and return readable text content (HTML tags stripped) |
| `extract_website` | Fetch a URL and extract only the main readable content (strips scripts, styles, nav, boilerplate) |
| `fetch_datetime` | Get the current date, time, timezone, and day of week |
| `fetch_geolocation` | Get geolocation data for an IP address (city, country, timezone, coordinates, ISP) via ip-api.com |
| `read_image` | Read an image from a file path or URL for vision analysis (PNG, JPEG, GIF, WebP, BMP, TIFF, SVG, ICO) |
| `generate_image` | Generate an image from a text description via GPT Image, Imagen, or Grok (auto-selects provider based on available keys; saves PNG to current directory) |
| `read_document` | Read a PDF, DOCX, or spreadsheet and extract content as markdown text. Supports `.pdf`, `.docx`, `.xlsx`, `.xls`, `.ods`. PDF text extracted directly; DOCX converted to markdown; spreadsheets converted to markdown tables (one per sheet) |
| `git` | Run a restricted `git` subcommand (no shell). Allows `status`, `diff`, `log`, `blame`, `commit` with a per-subcommand flag allowlist. Dangerous flags (`-c`, `-C`, `--ext-diff`, `--upload-pack`, `--exec-path`, `--no-verify`, `--amend`, `--git-dir`, `--work-tree`) and all other subcommands are rejected. Env vars that could redirect the subprocess (`GIT_DIR`, `GIT_SSH_COMMAND`, `GIT_CONFIG_*`, editor/askpass) are scrubbed |
| `run_code` | Execute a short code snippet in a chosen interpreter and return stdout/stderr. First line is the language (`python`, `node`, `ruby`, `perl`, `lua`, `bash`, `sh`); remaining lines are piped to the interpreter on stdin (no temp file). Useful for quick calculations, data transforms, and one-off logic checks. Shares the shell timeout, env scrubber, and CWD pin with `exec_shell`. Not a true sandbox |
| `lint_file` | Run a language-appropriate linter/formatter on a single file and return its diagnostics. Input is a file path; the linter is auto-selected from the extension (`.rs` → `rustfmt --check`, `.py` → `ruff`/`flake8`/`pyflakes`/`py_compile`, `.js`/`.ts` → `eslint`/`node --check`/`tsc`, `.go` → `gofmt`/`go vet`, `.sh` → `shellcheck`, `.rb` → `rubocop`/`ruby -c`, `.json` → `jq empty`, `.yaml` → `yamllint`, `.toml` → `taplo`, `.md` → `markdownlint`/`prettier`, `.lua` → `luacheck`, `.c`/`.cpp` → `clang-format`/`cppcheck`, `.html`/`.css` → `prettier`). The first candidate installed on `PATH` wins. No auto-fix — the file is never modified. Shares the shell timeout, env scrubber, and CWD pin with `exec_shell` |
| `test` | Run the project's test command and return a structured `Passed / Failed / Skipped` summary with per-failure detail. Empty body auto-detects the runner (cargo / npm / pytest / go / gradle / maven / ctest / make); a `<filter>` body narrows it; `--cmd <command>` overrides entirely. See [CODING_AGENT.md](CODING_AGENT.md) for how coding-agent mode threads test failures back into the loop |
| `json_query` | Query or transform JSON with jq-like expressions. First line is the jq filter (e.g. `.`, `.users[].name`, `.items \| length`, `map(select(.price > 10))`); remaining lines are inline JSON, or `@path/to/file.json` to load from a file in the working directory. Output is the pretty-printed filter result. Requires `jq` on `PATH` |
| `calculate` | Evaluate a math expression safely without any `eval` or shell subprocess. Supports int/float/scientific/hex/binary literals; `+ - * / %`, `^` / `**` (power); constants `pi`, `e`, `tau`; functions `sqrt`, `cbrt`, `abs`, `exp`, `ln`, `log2`, `log10`, `log`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `sinh`, `cosh`, `tanh`, `floor`, `ceil`, `round`, `trunc`, `sign`, `min`, `max`, `pow`, `atan2` |
| `csv_query` | Filter and project CSV/TSV with a SQL-like query language: `SELECT (* \| col, col, ...) FROM (csv \| tsv) [WHERE <cond> [AND\|OR <cond> ...]] [ORDER BY <col> [ASC\|DESC]] [LIMIT <N>]`. Inline CSV/TSV (with header row) or `@path/to/file.csv`. Output is a Markdown-style pipe table. Fully in-process |
| `list_processes` | List running processes with structured filtering. Invokes `ps` directly (no shell). Input is `key=value` pairs (empty = top 20 by %CPU): `name`, `user`, `pid`, `min_cpu`, `min_mem`, `port`, `sort=cpu\|mem\|pid\|name`, `limit` |
| `check_port` | Test whether a TCP port on a given host accepts connections. Pure tokio — no shell. Input is `<host>:<port> [timeout=<ms>]`; host may be DNS name, IPv4, or bracketed IPv6; an `http://` / `https://` URL is also accepted with the port inferred |
| `system_info` | Return structured OS, CPU, memory, and disk information as Markdown. Cross-platform for macOS and Linux. Input is optional `key=value` pairs: `section=os\|cpu\|memory\|disk\|all`, `path=<directory>` |
| `archive` | Create, extract, or list `tar.gz` / `tgz` / `tar` / `zip` archives in-process. Three modes: `create <format> <output>` followed by one input path per line; `extract <archive> <destination-dir>`; `list <archive>`. Refuses entries with `..` components, absolute paths, or symlinks (zip-slip / tar-slip guard) |
| `checksum` | Compute SHA-256 and/or MD5 cryptographic digests of a file. Input is a bare file path (returns both digests) or `sha256 <path>` / `md5 <path>` to pick one |
| `clipboard` | Read from or write to the system clipboard. `read` (or empty) to fetch the current clipboard contents, or `write` on the first line followed by the content on subsequent lines. Cross-platform: macOS uses `pbcopy` / `pbpaste`; Linux prefers Wayland with X11 fallback. Write size capped at 1 MB |
| `notify` | Send a desktop notification. First line is the title (required, max 256 bytes); remaining lines are the body (optional, max 4096 bytes). Cross-platform: macOS uses `osascript`; Linux uses `notify-send` |
| `view_map` | Display a map (OpenStreetMap or Esri satellite imagery) with one or more pins. Each input line is one pin: `<query>[ \| <label>[ \| <description>]]` where `<query>` is `<lat>,<lon>`, `<lat>,<lon>,<zoom>`, or a free-form place name (geocoded via Nominatim). Max 25 pins per call. **Renders only inside the aictl desktop app** — invoking from the CLI/terminal returns an error |
| `draw_chart` | Render a chart from structured data via Chart.js. Input is a single JSON object: `{"type":"line\|bar\|pie\|doughnut\|scatter","title","x_label","y_label","labels","series":[{"name","data"}]}`. Caps: 8 series, 500 points per series, 500 labels. **Renders only inside the aictl desktop app** — invoking from the CLI/terminal returns an error. Re-themes on the fly when the app theme flips |
| `save_memory` | Persist a fact about the user to long-term memory (`~/.aictl/memory.json`) so it survives across sessions. Auto-loaded into the system prompt of every future conversation under a `# Memory` block. Skipped silently in incognito mode or when `AICTL_MEMORY_ENABLED=false` |

## Image capabilities by provider

The `read_image` (vision/analysis) and `generate_image` tools depend on provider support:

| Provider | Image analysis (`read_image`) | Image generation (`generate_image`) |
|----------|-------------------------------|-------------------------------------|
| OpenAI | All models | GPT Image 1 / 1 Mini / 1.5 / 2 |
| Anthropic | All models | -- |
| Gemini | All models | Imagen 4.0 / 4.0 Ultra / 4.0 Fast |
| Grok | All models | Grok 2 Image / Grok Imagine Image / Grok Imagine Image Quality |
| Mistral | All models | -- |
| DeepSeek | -- | -- |
| Kimi | kimi-k2 series (k2.7 / k2.6 / k2.5 / k2-*) and moonshot-v1-*-vision-preview | -- |
| Z.ai | -- (requires GLM vision models not in catalog) | -- |
| Ollama | Model-dependent (e.g. llava, llama3.2-vision) | -- |

**Image generation fallback**: `generate_image` auto-selects a provider based on available API keys. The active provider is tried first (if it supports generation), then falls back through OpenAI, Gemini, and Grok in order. This means you can generate images even when your active chat provider (e.g. Anthropic or Mistral) doesn't offer a generation API — as long as you have at least one of `LLM_OPENAI_API_KEY`, `LLM_GEMINI_API_KEY`, or `LLM_GROK_API_KEY` configured.

## XML tool format

The tool-calling mechanism uses a custom XML format in the LLM response text (not provider-native tool APIs):

```xml
<tool name="exec_shell">
ls -la /tmp
</tool>
```

The agent loop runs for up to 20 iterations. LLM reasoning is printed to stderr; the final answer goes to stdout. Token usage, estimated cost, and execution time are always displayed after each response.

## Security

All tool calls pass through a configurable security policy (`crates/aictl-core/src/security.rs`) before execution. By default:

- **Shell command blocking**: dangerous commands are blocked (`rm`, `sudo`, `dd`, `mkfs`, `nc`, etc.). Command substitution (`$(...)`, backticks) is blocked. Compound commands (`|`, `&&`, `||`, `;`) are split and each segment is validated independently.
- **CWD jail**: file tools (`read_file`, `write_file`, `remove_file`, `edit_file`, `create_directory`, `list_directory`, `search_files`, `find_files`) can only operate within the working directory. Path traversal via `..` is defeated by canonicalization.
- **Blocked paths**: sensitive paths are always blocked (`~/.ssh`, `~/.gnupg`, `~/.aictl`, `~/.aws`, `~/.config/gcloud`, `/etc/shadow`, `/etc/sudoers`).
- **Environment scrubbing**: shell subprocesses receive a clean environment — vars matching `*_KEY`, `*_SECRET`, `*_TOKEN`, `*_PASSWORD` are stripped so API keys cannot leak.
- **Shell timeout**: commands are killed after 30 seconds (configurable).
- **Write size limit**: file writes are capped at 1 MB (configurable).
- **Output sanitization**: tool results are sanitized to prevent prompt injection via `<tool>` tags.
- **Injection guard**: user prompts are scanned before being sent to the LLM. Inputs containing instruction-override phrases ("ignore previous instructions", "disable security", etc.) or forged role/tool tags (`<tool …>`, `<|system|>`, `### System:`, etc.) are blocked with a clear error. Disable with `AICTL_SECURITY_INJECTION_GUARD=false`.
- **Audit log**: every tool invocation appends one JSON line to `~/.aictl/audit/<session-id>` (JSONL) with timestamp, tool name, truncated input, and an outcome tag (`executed` + `result_summary`, `denied_by_policy` + `reason`, `denied_by_user`, `disabled`, `duplicate`) — separate from session history so a reviewer can reconstruct exactly what the model ran. Skipped in incognito mode and single-shot runs. Disable with `AICTL_SECURITY_AUDIT_LOG=false`.
- **Sensitive-data redaction** (opt-in): every outbound message body can be screened for credentials and PII before it reaches a remote provider. Enable with `AICTL_SECURITY_REDACTION=redact` to swap matches for `[REDACTED:<KIND>]` on the wire, or `=block` to abort the turn on any hit. Layer A: regex detectors for API keys (OpenAI / Anthropic / Google / GitHub / Stripe / Slack / HuggingFace / Groq), AWS access keys, JWTs (with base64-header sanity check), PEM private keys, DB/AMQP connection strings, emails, context-gated phones, credit cards (Luhn), IBANs (mod-97). Layer B: Shannon-entropy scanner for opaque tokens. Layer C (optional `redaction-ner` cargo feature + pulled GLiNER model): person / location / organization detection. User-supplied `AICTL_REDACTION_EXTRA_PATTERNS` and `AICTL_REDACTION_ALLOW` tune the detectors. Local providers (Ollama / GGUF / MLX) bypass by default. Every redaction event lands in the audit log; the persisted session file always keeps the user's original text.

Security denials are returned to the LLM as tool results (displayed in red) so it can adapt. Use `--unrestricted` to disable all security checks. Individual settings are configurable via `AICTL_SECURITY_*` keys in `~/.aictl/config` (see [CONFIG.md](CONFIG.md#security-configuration)). The audit log and redaction layer are observability and privacy controls, not tool-call enforcement, so `--unrestricted` leaves them running unless the config key turns them off.
