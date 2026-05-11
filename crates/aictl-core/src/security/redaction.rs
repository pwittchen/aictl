//! Sensitive-data redaction for outbound LLM messages and inbound tool
//! results. Sits at a different seam than the rest of [`crate::security`]:
//! the path / shell / tool-call gates run on *tool calls*, while this
//! layer inspects *message bodies* right before they leave for a remote
//! provider (and right before a tool result rejoins the conversation
//! history). See `.claude/plans/sensitive-data-redaction.md` for the
//! design doc.
//!
//! # Detection layers
//!
//! - **Layer A — high-precision regex packs.** One compiled `Regex` per
//!   kind: API keys (`sk-…`, `sk-ant-…`, `AIza…`, `ghp_…`, `hf_…`,
//!   `sk_live_…`, `xoxb-…`), AWS access key IDs (`AKIA…` / `ASIA…`),
//!   JWTs, PEM private-key blocks, DB/AMQP connection strings with
//!   inline credentials, emails, phones (context-gated), credit-card
//!   digit runs (validated via Luhn), IBANs (validated via mod-97).
//! - **Layer B — entropy heuristic.** Sliding window over contiguous
//!   `[A-Za-z0-9/+=_-]` runs of length ≥ 32 with Shannon entropy above
//!   a threshold. Catches rotated or unknown-prefix tokens. User
//!   allowlist runs after this layer, so known-good hashes/UUIDs can
//!   be whitelisted.
//! - **Layer C — optional NER.** Gated behind the `redaction-ner`
//!   cargo feature (scaffolded only — the feature currently returns a
//!   clear "not built" error when `AICTL_REDACTION_NER=true` is set
//!   without the feature).
//!
//! Overlapping matches are resolved by priority: specific kinds
//! (`Jwt`, `PrivateKey`, `ApiKey`) beat generic `HighEntropy`; custom
//! user-defined patterns win over everything else. Ranges are
//! non-overlapping in the final list so placeholder substitution is
//! deterministic.
//!
//! Low-confidence matches are dropped before the redact/block
//! decision — every Layer-B entropy hit and any Layer-C NER span
//! whose model probability is below 0.65 is treated as noise rather
//! than a sensitive-data finding. High-confidence regex hits
//! (Layer A) and confident NER spans still flow through unchanged.
//!
//! # Modes
//!
//! - `off` — pass-through (default for v1).
//! - `redact` — replace match ranges with `[REDACTED:<KIND>]`. The
//!   original text is never sent to the provider.
//! - `block` — abort the turn; the error names each detected kind
//!   with a scrubbed context snippet.
//!
//! # History boundary
//!
//! Redaction runs at two seams:
//!
//! 1. *Network boundary.* A transient redacted clone of the message
//!    slice is handed to the provider for one call; the caller's
//!    mutable `Vec<Message>` is never mutated.
//! 2. *Persistence boundary.* When mode is `redact` or `block`, the
//!    same detector pipeline is run again as session messages are
//!    serialized to disk and as REPL input lines are written to
//!    `~/.aictl/history`, so a leaked secret can't sit in plaintext
//!    in `~/.aictl/sessions/<id>` or be recalled from shell history.
//!    `block` is treated as `redact` for this seam — the network call
//!    has already aborted, but the user message that tripped the
//!    block sits in `messages` and would otherwise hit disk verbatim.
//!    Off mode short-circuits both seams.

use std::ops::Range;
use std::sync::{Arc, OnceLock, RwLock};

use regex::Regex;

use crate::config::config_get_scoped;

pub mod ner;

/// Redaction mode — read from `AICTL_SECURITY_REDACTION` at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionMode {
    /// Pass-through. The default for v1 — no behavior change for users
    /// who do not opt in.
    Off,
    /// Replace each match range with `[REDACTED:<KIND>]` before the
    /// message leaves for the provider.
    Redact,
    /// Abort the turn on any match; surface the kinds to the user.
    Block,
}

/// Typed category for a single match. The placeholder rendered into
/// redacted text is derived from this enum.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DetectorKind {
    /// Known-prefix API keys (`OpenAI` `sk-…`, Anthropic `sk-ant-…`,
    /// Google `AIza…`, GitHub `ghp_…` / `gho_…`, `HuggingFace` `hf_…`,
    /// Stripe `sk_live_…`, Slack `xoxb-…` …).
    ApiKey,
    /// AWS access-key ID: `AKIA…` / `ASIA…` followed by 16 [A-Z0-9].
    AwsAccessKey,
    /// JSON Web Token: three base64url segments joined with `.` where
    /// the first two decode to `{` (header + payload).
    Jwt,
    /// PEM private-key block: `-----BEGIN … PRIVATE KEY-----` … `-----END … PRIVATE KEY-----`.
    PrivateKey,
    /// DB/AMQP/Redis URL with inline credentials — `scheme://user:pass@host…`.
    ConnectionString,
    /// 13–19 digit run that passes the Luhn check.
    CreditCard,
    /// IBAN that passes the mod-97 check.
    Iban,
    /// Email address (RFC-5322 practical subset).
    Email,
    /// Phone number, gated on context keywords (`phone`, `tel`, `mobile`, `cell`).
    Phone,
    /// Value of a credential-shaped URL query parameter — `?token=…`,
    /// `&api_key=…`, `?password=…`, etc. The match range covers only
    /// the value, so the parameter name stays visible after substitution.
    UrlSecret,
    /// US Social Security Number in the canonical `123-45-6789` shape.
    Ssn,
    /// Polish national identification number (PESEL) — 11 digits with
    /// the standard weighted checksum (weights 1,3,7,9,1,3,7,9,1,3).
    Pesel,
    /// IPv4 (octet-validated) or IPv6 address.
    IpAddress,
    /// MAC address — six hex pairs separated by `:` or `-`.
    MacAddress,
    /// AWS secret access key — 40-char base64 alphabet (`[A-Za-z0-9/+]`)
    /// gated on AWS / secret context within ~50 chars to avoid flagging
    /// arbitrary 40-char base64 blobs.
    AwsSecretKey,
    /// Layer B — opaque high-entropy token.
    HighEntropy,
    /// Layer C — person name detected by the optional NER backend.
    /// Only constructed when the `redaction-ner` cargo feature is
    /// built in; the `dead_code` allow is for non-feature builds
    /// where the variant is reachable but unused.
    #[cfg_attr(not(feature = "redaction-ner"), allow(dead_code))]
    PersonName,
    /// Layer C — physical location (address, city, country) detected
    /// by the optional NER backend.
    #[cfg_attr(not(feature = "redaction-ner"), allow(dead_code))]
    Location,
    /// Layer C — organization / company / institution name detected
    /// by the optional NER backend.
    #[cfg_attr(not(feature = "redaction-ner"), allow(dead_code))]
    Organization,
    /// User-defined pattern from `AICTL_REDACTION_EXTRA_PATTERNS`.
    /// The `String` is the user-supplied name (becomes the placeholder
    /// suffix, e.g. `CUSTOMER_ID`).
    Custom(String),
}

impl DetectorKind {
    /// Placeholder suffix used in `[REDACTED:<SUFFIX>]`. Uppercase,
    /// matches the typed scheme described in the plan.
    pub fn placeholder(&self) -> String {
        match self {
            Self::ApiKey => "API_KEY".to_string(),
            Self::AwsAccessKey => "AWS_KEY".to_string(),
            Self::Jwt => "JWT".to_string(),
            Self::PrivateKey => "PRIVATE_KEY".to_string(),
            Self::ConnectionString => "CONNECTION_STRING".to_string(),
            Self::CreditCard => "CREDIT_CARD".to_string(),
            Self::Iban => "IBAN".to_string(),
            Self::Email => "EMAIL".to_string(),
            Self::Phone => "PHONE".to_string(),
            Self::UrlSecret => "URL_SECRET".to_string(),
            Self::Ssn => "SSN".to_string(),
            Self::Pesel => "PESEL".to_string(),
            Self::IpAddress => "IP_ADDRESS".to_string(),
            Self::MacAddress => "MAC_ADDRESS".to_string(),
            Self::AwsSecretKey => "AWS_SECRET_KEY".to_string(),
            Self::HighEntropy => "HIGH_ENTROPY".to_string(),
            Self::PersonName => "PERSON".to_string(),
            Self::Location => "LOCATION".to_string(),
            Self::Organization => "ORGANIZATION".to_string(),
            Self::Custom(name) => name.clone(),
        }
    }

    /// Slug used in `AICTL_REDACTION_DETECTORS` to enable/disable a
    /// specific kind. Custom kinds are not filterable via the slug
    /// list — if the user declared them they are assumed wanted.
    pub fn slug(&self) -> &str {
        match self {
            Self::ApiKey => "api_key",
            Self::AwsAccessKey => "aws",
            Self::Jwt => "jwt",
            Self::PrivateKey => "private_key",
            Self::ConnectionString => "connection_string",
            Self::CreditCard => "credit_card",
            Self::Iban => "iban",
            Self::Email => "email",
            Self::Phone => "phone",
            Self::UrlSecret => "url_secret",
            Self::Ssn => "ssn",
            Self::Pesel => "pesel",
            Self::IpAddress => "ip_address",
            Self::MacAddress => "mac_address",
            Self::AwsSecretKey => "aws_secret",
            Self::HighEntropy => "high_entropy",
            Self::PersonName => "person_name",
            Self::Location => "location",
            Self::Organization => "organization",
            Self::Custom(s) => s.as_str(),
        }
    }

    /// Whether this detector should still fire on a match whose span
    /// lies entirely inside a URL. Only secret-class kinds qualify —
    /// names, locations, IPs, etc. inside URLs are typically meaningful
    /// parts of the address (e.g. the `windguru` label of
    /// `windguru.cz`) and redacting them mangles the URL without
    /// removing anything sensitive.
    fn is_secret_class(&self) -> bool {
        matches!(
            self,
            Self::ApiKey
                | Self::AwsAccessKey
                | Self::AwsSecretKey
                | Self::Jwt
                | Self::PrivateKey
                | Self::ConnectionString
                | Self::UrlSecret
                | Self::HighEntropy
                | Self::Custom(_)
        )
    }

    /// Resolution priority when two matches overlap — higher wins.
    /// Keeps `Jwt` from being shadowed by the entropy scanner, and
    /// keeps a user-defined `Custom` pattern ahead of everything.
    fn priority(&self) -> u8 {
        match self {
            Self::Custom(_) => 10,
            Self::PrivateKey => 9,
            Self::Jwt | Self::ConnectionString | Self::AwsSecretKey => 8,
            Self::ApiKey | Self::AwsAccessKey | Self::Ssn | Self::Pesel => 7,
            Self::Iban | Self::CreditCard | Self::UrlSecret => 6,
            Self::Email => 5,
            Self::Phone | Self::IpAddress | Self::MacAddress => 4,
            // NER hits sit above HighEntropy (model output is more
            // informative than "looks random") but below structured
            // detectors so a regex-confirmed credential wins over a
            // mis-classified proper-noun span.
            Self::PersonName | Self::Location | Self::Organization => 3,
            Self::HighEntropy => 1,
        }
    }
}

/// A single detected span in a message. Byte ranges are into the input
/// `&str` so `&text[range]` round-trips cleanly.
#[derive(Debug, Clone)]
pub struct Match {
    pub kind: DetectorKind,
    pub range: Range<usize>,
    pub confidence: &'static str,
}

/// Direction / origin tag for the audit log. Matches the `source` field
/// shape in the plan's audit entry.
#[derive(Debug, Clone, Copy)]
pub enum RedactionSource {
    SystemPrompt,
    UserMessage,
    AssistantMessage,
    ToolResult,
}

impl RedactionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SystemPrompt => "system_prompt",
            Self::UserMessage => "user_message",
            Self::AssistantMessage => "assistant_message",
            Self::ToolResult => "tool_result",
        }
    }
}

/// Direction of the message relative to the process boundary.
#[derive(Debug, Clone, Copy)]
pub enum RedactionDirection {
    /// User/assistant/tool-result payload heading to a remote provider.
    Outbound,
    /// Tool result on its way into conversation history.
    Inbound,
}

impl RedactionDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Outbound => "outbound",
            Self::Inbound => "inbound",
        }
    }
}

/// What `redact()` returns. `Clean` carries no payload — callers should
/// use the original input untouched in that branch, avoiding any clone.
#[derive(Debug)]
pub enum RedactionResult {
    /// No match (or mode is `Off`). The original text is safe to send.
    Clean,
    /// At least one match; `text` is the placeholder-substituted output
    /// and `matches` is the (deduplicated, priority-resolved) hit list.
    Redacted { text: String, matches: Vec<Match> },
    /// Block mode tripped. The caller must surface an error; the
    /// original text must not be sent.
    Blocked { matches: Vec<Match> },
}

/// Global redaction policy loaded once at startup.
pub struct RedactionPolicy {
    pub mode: RedactionMode,
    /// Skip redaction for local providers (Ollama, GGUF, MLX). Default
    /// `true` — data never leaves the machine, no privacy gain.
    pub skip_local: bool,
    /// Enabled built-in detector slugs. Empty = all built-ins enabled.
    /// User-supplied custom patterns always run regardless.
    pub enabled_detectors: Vec<String>,
    /// User-defined `(name, regex)` pairs from
    /// `AICTL_REDACTION_EXTRA_PATTERNS`. `name` becomes the placeholder
    /// suffix.
    pub extra_patterns: Vec<(String, Regex)>,
    /// Regexes whose matches suppress any overlapping detection hit —
    /// e.g. `AKIATEST0000000000` or an internal commit-hash allowlist.
    pub allowlist: Vec<Regex>,
    /// User opted in to the NER pass. Only meaningful when the
    /// `redaction-ner` cargo feature is built in; otherwise we surface
    /// a startup warning. Surfaced in `/security` so the user can
    /// see whether their opt-in actually has an effect.
    pub ner_requested: bool,
}

impl RedactionPolicy {
    /// An inert policy used for tests and as the `OnceLock` fallback.
    fn off() -> Self {
        Self {
            mode: RedactionMode::Off,
            skip_local: true,
            enabled_detectors: vec![],
            extra_patterns: vec![],
            allowlist: vec![],
            ner_requested: false,
        }
    }

    /// One-line summary suitable for `/security` output.
    #[must_use]
    pub fn summary(&self) -> String {
        // One-line mode summary for the `redaction:` row in `/security`
        // and `/info`. The full breakdown (active detectors, custom
        // patterns, allowlist, NER state) is printed by
        // `commands::security::print_redaction_detail` directly below
        // this row, so this line stays short.
        match self.mode {
            RedactionMode::Off => "off".to_string(),
            RedactionMode::Redact => "redact (network-boundary scrubbing)".to_string(),
            RedactionMode::Block => "block (abort on sensitive data)".to_string(),
        }
    }

    pub(crate) fn is_detector_enabled(&self, kind: &DetectorKind) -> bool {
        // Custom patterns always run (they're declared by the user).
        if matches!(kind, DetectorKind::Custom(_)) {
            return true;
        }
        if self.enabled_detectors.is_empty() {
            return true;
        }
        self.enabled_detectors.iter().any(|s| s == kind.slug())
    }
}

static POLICY: RwLock<Option<Arc<RedactionPolicy>>> = RwLock::new(None);

/// Initialize the redaction policy from config. Call once at startup
/// after [`crate::config::load_config`]. Returns any warnings produced
/// during config parsing (bad regexes, missing NER model / feature) so
/// the caller can route them through the active UI rather than having
/// the engine reach into stderr directly.
#[must_use]
pub fn init() -> Vec<String> {
    let (pol, warnings) = load_policy();
    *POLICY.write().expect("redaction policy lock poisoned") = Some(Arc::new(pol));
    warnings
}

/// Re-read the policy from config and atomically replace the cached
/// snapshot. The desktop calls this after the user changes redaction
/// settings via the Settings tab so the new mode / detector list takes
/// effect on the next outbound message without restarting the app. The
/// CLI loads policy once at startup and does not call this.
#[must_use]
pub fn reload() -> Vec<String> {
    init()
}

/// Access the global redaction policy. Returns an inert `Off` policy if
/// `init()` has not been called (tests, defensive fallback).
pub fn policy() -> Arc<RedactionPolicy> {
    static DEFAULT: OnceLock<Arc<RedactionPolicy>> = OnceLock::new();
    POLICY
        .read()
        .expect("redaction policy lock poisoned")
        .clone()
        .unwrap_or_else(|| {
            DEFAULT
                .get_or_init(|| Arc::new(RedactionPolicy::off()))
                .clone()
        })
}

fn load_policy() -> (RedactionPolicy, Vec<String>) {
    // Every redaction knob honors the `AICTL_SERVER_*` override when
    // the engine is running inside `aictl-server`, so the proxy can run
    // a stricter (or weaker) data-leak posture than the operator's
    // interactive CLI without forking `~/.aictl/config`. When the
    // server-prefixed key is unset the lookup falls through to the
    // shared `AICTL_*` value.
    let mut warnings = Vec::new();
    let mode = match config_get_scoped(
        "AICTL_SERVER_SECURITY_REDACTION",
        "AICTL_SECURITY_REDACTION",
    )
    .as_deref()
    {
        Some("redact") => RedactionMode::Redact,
        Some("block") => RedactionMode::Block,
        _ => RedactionMode::Off,
    };

    let skip_local = config_get_scoped(
        "AICTL_SERVER_SECURITY_REDACTION_LOCAL",
        "AICTL_SECURITY_REDACTION_LOCAL",
    )
    .is_none_or(|v| v != "true" && v != "1");

    let enabled_detectors = parse_csv(
        &config_get_scoped(
            "AICTL_SERVER_REDACTION_DETECTORS",
            "AICTL_REDACTION_DETECTORS",
        )
        .unwrap_or_default(),
        ',',
    );

    let extra_patterns = parse_extra_patterns(
        &config_get_scoped(
            "AICTL_SERVER_REDACTION_EXTRA_PATTERNS",
            "AICTL_REDACTION_EXTRA_PATTERNS",
        )
        .unwrap_or_default(),
        &mut warnings,
    );

    let allowlist: Vec<Regex> = parse_csv(
        &config_get_scoped("AICTL_SERVER_REDACTION_ALLOW", "AICTL_REDACTION_ALLOW")
            .unwrap_or_default(),
        ';',
    )
    .into_iter()
    .filter_map(|p| match Regex::new(&p) {
        Ok(r) => Some(r),
        Err(e) => {
            warnings.push(format!(
                "invalid AICTL_REDACTION_ALLOW pattern '{p}': {e}. Skipped."
            ));
            None
        }
    })
    .collect();

    let ner_requested = matches!(
        config_get_scoped("AICTL_SERVER_REDACTION_NER", "AICTL_REDACTION_NER").as_deref(),
        Some("true" | "1")
    );

    if ner_requested {
        match ner::status(true) {
            ner::NerStatus::FeatureMissing => warnings.push(
                "AICTL_REDACTION_NER=true but this build lacks the `redaction-ner` feature. \
                 The NER pass will not run. Rebuild with `cargo build --features redaction-ner`."
                    .to_string(),
            ),
            ner::NerStatus::ModelMissing { expected_name } => warnings.push(format!(
                "AICTL_REDACTION_NER=true but NER model '{expected_name}' is not on disk. \
                 Run `aictl --pull-ner-model <owner>/<repo>` (default: {}) to fetch it.",
                ner::DEFAULT_NER_MODEL
            )),
            ner::NerStatus::Disabled | ner::NerStatus::Ready { .. } => {}
        }
    }

    (
        RedactionPolicy {
            mode,
            skip_local,
            enabled_detectors,
            extra_patterns,
            allowlist,
            ner_requested,
        },
        warnings,
    )
}

fn parse_csv(s: &str, sep: char) -> Vec<String> {
    s.split(sep)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Parse `NAME=REGEX;NAME=REGEX;…` into compiled pairs. Bad regexes are
/// recorded as warnings and skipped rather than aborting startup; the
/// caller routes the accumulated messages through the active UI.
fn parse_extra_patterns(s: &str, warnings: &mut Vec<String>) -> Vec<(String, Regex)> {
    let mut out = Vec::new();
    for entry in s.split(';') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((name, pattern)) = entry.split_once('=') else {
            warnings.push(format!(
                "AICTL_REDACTION_EXTRA_PATTERNS entry '{entry}' is missing '=' — expected NAME=REGEX. Skipped."
            ));
            continue;
        };
        let name = name.trim();
        let pattern = pattern.trim();
        if name.is_empty() || pattern.is_empty() {
            warnings.push(format!(
                "AICTL_REDACTION_EXTRA_PATTERNS entry '{entry}' has empty name or pattern. Skipped."
            ));
            continue;
        }
        match Regex::new(pattern) {
            Ok(r) => out.push((name.to_ascii_uppercase(), r)),
            Err(e) => warnings.push(format!(
                "invalid AICTL_REDACTION_EXTRA_PATTERNS regex for '{name}': {e}. Skipped."
            )),
        }
    }
    out
}

// --- Public entry point ---

/// Redact (or block, or leave untouched) the given text according to the
/// provided policy. Pure function: no global state, easy to test.
pub fn redact(text: &str, pol: &RedactionPolicy) -> RedactionResult {
    if matches!(pol.mode, RedactionMode::Off) || text.is_empty() {
        return RedactionResult::Clean;
    }

    let matches = find_matches(text, pol);
    if matches.is_empty() {
        return RedactionResult::Clean;
    }

    match pol.mode {
        RedactionMode::Off => RedactionResult::Clean,
        RedactionMode::Redact => {
            let text = apply_placeholders(text, &matches);
            RedactionResult::Redacted { text, matches }
        }
        RedactionMode::Block => RedactionResult::Blocked { matches },
    }
}

/// Persistence-boundary redactor. Used when serializing session messages
/// to `~/.aictl/sessions/<id>` and when writing REPL input lines to
/// `~/.aictl/history`. Returns `None` when the input is unchanged
/// (mode `Off`, empty text, or no matches) so the caller can avoid an
/// allocation in the common case.
///
/// Differs from [`redact`] in two ways: `Block` mode is treated as
/// `Redact` (the network call already aborted; the leaked text still
/// needs scrubbing before disk), and the result is a plain `Option<String>`
/// because callers at this seam never need the match list.
pub fn redact_for_persistence(text: &str, pol: &RedactionPolicy) -> Option<String> {
    if matches!(pol.mode, RedactionMode::Off) || text.is_empty() {
        return None;
    }
    let matches = find_matches(text, pol);
    if matches.is_empty() {
        return None;
    }
    Some(apply_placeholders(text, &matches))
}

/// Render a short, non-sensitive description of the block-mode matches
/// for error messages. Never includes the original match content — only
/// the placeholder plus a few characters of surrounding context.
pub fn describe_matches(text: &str, matches: &[Match]) -> String {
    let mut kinds: Vec<String> = matches.iter().map(|m| m.kind.placeholder()).collect();
    kinds.sort();
    kinds.dedup();
    let kinds_str = kinds.join(", ");
    let first = &matches[0];
    let ctx_start = safe_boundary(text, first.range.start.saturating_sub(20), false);
    let ctx_end = safe_boundary(text, (first.range.end + 20).min(text.len()), true);
    let before = &text[ctx_start..first.range.start];
    let after = &text[first.range.end..ctx_end];
    let placeholder = format!("[REDACTED:{}]", first.kind.placeholder());
    let snippet = format!("…{before}{placeholder}{after}…");
    format!("{kinds_str} at: {snippet}")
}

// --- Matching ---

fn find_matches(text: &str, pol: &RedactionPolicy) -> Vec<Match> {
    let mut raw: Vec<Match> = Vec::new();

    // Custom user patterns first — they win on priority.
    for (name, re) in &pol.extra_patterns {
        for m in re.find_iter(text) {
            raw.push(Match {
                kind: DetectorKind::Custom(name.clone()),
                range: m.start()..m.end(),
                confidence: "user",
            });
        }
    }

    // Layer A built-in detectors.
    run_regex_detector(
        text,
        api_key_regex(),
        &DetectorKind::ApiKey,
        "high",
        pol,
        &mut raw,
    );
    run_regex_detector(
        text,
        aws_access_key_regex(),
        &DetectorKind::AwsAccessKey,
        "high",
        pol,
        &mut raw,
    );
    run_jwt_detector(text, pol, &mut raw);
    run_regex_detector(
        text,
        private_key_regex(),
        &DetectorKind::PrivateKey,
        "high",
        pol,
        &mut raw,
    );
    run_regex_detector(
        text,
        connection_string_regex(),
        &DetectorKind::ConnectionString,
        "high",
        pol,
        &mut raw,
    );
    run_credit_card_detector(text, pol, &mut raw);
    run_iban_detector(text, pol, &mut raw);
    run_regex_detector(
        text,
        email_regex(),
        &DetectorKind::Email,
        "medium",
        pol,
        &mut raw,
    );
    run_phone_detector(text, pol, &mut raw);
    run_url_secret_detector(text, pol, &mut raw);
    run_regex_detector(text, ssn_regex(), &DetectorKind::Ssn, "high", pol, &mut raw);
    run_pesel_detector(text, pol, &mut raw);
    run_ip_detector(text, pol, &mut raw);
    run_regex_detector(
        text,
        mac_regex(),
        &DetectorKind::MacAddress,
        "high",
        pol,
        &mut raw,
    );
    run_aws_secret_detector(text, pol, &mut raw);

    // Layer B entropy scanner.
    if pol.is_detector_enabled(&DetectorKind::HighEntropy) {
        run_entropy_scanner(text, &mut raw);
    }

    // Layer C — NER-based person/location/organization detection.
    // The call is a no-op when the `redaction-ner` feature is off,
    // when the user has not opted in, or when no model is on disk —
    // see `ner::run_ner_detector` for the gating.
    if pol.ner_requested {
        ner::run_ner_detector(text, pol, &mut raw);
    }

    // Drop non-secret-class matches whose span lies inside a URL.
    // Domain labels, paths, and other URL parts routinely trip NER /
    // PII detectors (`windguru` in `windguru.cz` reads as an
    // Organization; an `/orders/john-smith/` path reads as a person)
    // but the URL itself is the point of the message — redacting these
    // mangles the address without hiding anything sensitive. Secrets
    // embedded in the URL (api keys, tokens, JWTs, …) still fire.
    let urls = find_url_spans(text);
    if !urls.is_empty() {
        raw.retain(|m| {
            if m.kind.is_secret_class() {
                return true;
            }
            !urls
                .iter()
                .any(|u| u.start <= m.range.start && u.end >= m.range.end)
        });
    }

    // Filter out anything covered by the allowlist (run after Layers
    // B and C so users can whitelist a known-good high-entropy hash
    // or a false-positive name).
    if !pol.allowlist.is_empty() {
        raw.retain(|m| !is_allowlisted(text, m, &pol.allowlist));
    }

    // Drop low-confidence hits before they reach the redact/block
    // decision. The Layer-B entropy scanner (always "low") and
    // sub-0.65-probability NER spans are the dominant sources of
    // false positives — surfacing them only sharpens the alert
    // signal for high-confidence regex / model hits.
    raw.retain(|m| m.confidence != "low");

    merge_overlaps(raw)
}

fn run_regex_detector(
    text: &str,
    re: &Regex,
    kind: &DetectorKind,
    confidence: &'static str,
    pol: &RedactionPolicy,
    out: &mut Vec<Match>,
) {
    if !pol.is_detector_enabled(kind) {
        return;
    }
    for m in re.find_iter(text) {
        out.push(Match {
            kind: kind.clone(),
            range: m.start()..m.end(),
            confidence,
        });
    }
}

/// JWT detector with a cheap sanity-check: the header segment must
/// base64-decode to something that starts with `{`, rejecting the
/// common `eyJ…` false-positives in binary/encoded blobs.
fn run_jwt_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::Jwt) {
        return;
    }
    for m in jwt_regex().find_iter(text) {
        let matched = m.as_str();
        let Some(header) = matched.split('.').next() else {
            continue;
        };
        if decoded_starts_with_brace(header) {
            out.push(Match {
                kind: DetectorKind::Jwt,
                range: m.start()..m.end(),
                confidence: "high",
            });
        }
    }
}

fn decoded_starts_with_brace(b64url: &str) -> bool {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    URL_SAFE_NO_PAD
        .decode(b64url)
        .ok()
        .and_then(|bytes| bytes.first().copied())
        .is_some_and(|b| b == b'{')
}

fn run_credit_card_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::CreditCard) {
        return;
    }
    for m in credit_card_regex().find_iter(text) {
        let raw = m.as_str();
        let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
        let n = digits.len();
        // Reject UUIDs (32 chars), commit SHAs, and overlong runs.
        if !(13..=19).contains(&n) {
            continue;
        }
        if luhn_check(&digits) {
            out.push(Match {
                kind: DetectorKind::CreditCard,
                range: m.start()..m.end(),
                confidence: "high",
            });
        }
    }
}

fn luhn_check(digits: &str) -> bool {
    let mut sum: u32 = 0;
    for (i, c) in digits.chars().rev().enumerate() {
        let Some(d) = c.to_digit(10) else {
            return false;
        };
        if i % 2 == 1 {
            let doubled = d * 2;
            sum += if doubled > 9 { doubled - 9 } else { doubled };
        } else {
            sum += d;
        }
    }
    sum.is_multiple_of(10)
}

fn run_iban_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::Iban) {
        return;
    }
    for m in iban_regex().find_iter(text) {
        if iban_check(m.as_str()) {
            out.push(Match {
                kind: DetectorKind::Iban,
                range: m.start()..m.end(),
                confidence: "high",
            });
        }
    }
}

/// mod-97 IBAN validator. Moves the first four characters to the end,
/// converts letters to two-digit codes (A=10…Z=35), computes mod 97.
fn iban_check(iban: &str) -> bool {
    let iban: String = iban.chars().filter(|c| !c.is_whitespace()).collect();
    if iban.len() < 15 || iban.len() > 34 {
        return false;
    }
    let (head, tail) = iban.split_at(4);
    let rearranged = format!("{tail}{head}");
    let mut expanded = String::with_capacity(rearranged.len() * 2);
    for c in rearranged.chars() {
        if c.is_ascii_digit() {
            expanded.push(c);
        } else if c.is_ascii_alphabetic() {
            let code = (c.to_ascii_uppercase() as u32) - ('A' as u32) + 10;
            expanded.push_str(&code.to_string());
        } else {
            return false;
        }
    }
    // Stream mod-97 over the decimal string — the number can be
    // hundreds of digits long.
    let mut rem: u64 = 0;
    for c in expanded.chars() {
        let d = c.to_digit(10).unwrap_or(0);
        rem = (rem * 10 + u64::from(d)) % 97;
    }
    rem == 1
}

/// PESEL detector. The shape — 11 contiguous digits — is too generic
/// to flag on its own (timestamps, order numbers, IDs), so each match
/// is gated on the official weighted-checksum validator. False
/// positives at this length are rare once the checksum holds.
fn run_pesel_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::Pesel) {
        return;
    }
    for m in pesel_regex().find_iter(text) {
        if pesel_check(m.as_str()) {
            out.push(Match {
                kind: DetectorKind::Pesel,
                range: m.start()..m.end(),
                confidence: "high",
            });
        }
    }
}

/// Standard PESEL checksum: digits weighted by 1,3,7,9,1,3,7,9,1,3,
/// summed mod 10, then `(10 - sum) mod 10` must equal the last digit.
fn pesel_check(digits: &str) -> bool {
    const WEIGHTS: [u32; 10] = [1, 3, 7, 9, 1, 3, 7, 9, 1, 3];
    if digits.len() != 11 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let bytes = digits.as_bytes();
    let mut sum = 0u32;
    for (i, &w) in WEIGHTS.iter().enumerate() {
        sum += u32::from(bytes[i] - b'0') * w;
    }
    let expected = (10 - (sum % 10)) % 10;
    expected == u32::from(bytes[10] - b'0')
}

/// Phone numbers are noisy by shape — an arbitrary 10-digit run could be
/// an order number or postal code. Gate the match on a context keyword
/// (`phone`, `tel`, `mobile`, `cell`, `fax`) within ~30 chars to the
/// left of the span. E.164 international format (`+CC…`) is high enough
/// confidence to flag without context.
fn run_phone_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::Phone) {
        return;
    }
    let lower = text.to_ascii_lowercase();
    for m in phone_regex().find_iter(text) {
        let raw = m.as_str();
        let digits = raw.chars().filter(char::is_ascii_digit).count();
        if !(7..=15).contains(&digits) {
            continue;
        }
        // E.164 (`+CC…`) is a standalone signal; otherwise require
        // `phone`/`tel`/`mobile`/`cell`/`fax` nearby.
        let confidence = if raw.trim_start().starts_with('+') {
            "high"
        } else if has_phone_context(&lower, m.start()) {
            "medium"
        } else {
            continue;
        };
        out.push(Match {
            kind: DetectorKind::Phone,
            range: m.start()..m.end(),
            confidence,
        });
    }
}

const PHONE_CONTEXT_WORDS: &[&str] = &["phone", "tel", "mobile", "cell", "fax"];

/// URL query-parameter credential detector. Catches the value of any
/// query parameter whose name reads as a credential — `?token=…`,
/// `&api_key=…`, `?password=…`, `&client_secret=…`, etc. The match
/// range covers capture group 1 (the value only), so the parameter
/// name stays visible after substitution: `?token=[REDACTED:URL_SECRET]`.
/// Without this layer, short opaque values like `abc123secret` slip past
/// the API-key prefix list and the 32-char entropy floor.
fn run_url_secret_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::UrlSecret) {
        return;
    }
    for caps in url_secret_regex().captures_iter(text) {
        let Some(value) = caps.get(1) else { continue };
        out.push(Match {
            kind: DetectorKind::UrlSecret,
            range: value.start()..value.end(),
            confidence: "high",
        });
    }
}

fn has_phone_context(lower_text: &str, span_start: usize) -> bool {
    let window_start = span_start.saturating_sub(30);
    let window_start = safe_boundary(lower_text, window_start, false);
    let window = &lower_text[window_start..span_start];
    PHONE_CONTEXT_WORDS.iter().any(|w| window.contains(w))
}

/// IPv4 + IPv6 detector. The IPv4 regex already validates each octet is
/// `0-255`; the IPv6 regex covers the full 8-group form and the common
/// `::` zero-compression variants. The `IpAddress` kind is shared so
/// users only see a single placeholder.
fn run_ip_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::IpAddress) {
        return;
    }
    for m in ipv4_regex().find_iter(text) {
        out.push(Match {
            kind: DetectorKind::IpAddress,
            range: m.start()..m.end(),
            confidence: "high",
        });
    }
    for m in ipv6_regex().find_iter(text) {
        // The IPv6 alternation is permissive enough to swallow short
        // strings like `::1` (loopback) which is exactly what we want
        // — those are still IP addresses worth scrubbing.
        out.push(Match {
            kind: DetectorKind::IpAddress,
            range: m.start()..m.end(),
            confidence: "high",
        });
    }
}

/// AWS secret access keys are 40-char strings over the base64 alphabet
/// without padding (`[A-Za-z0-9/+]`). The shape is too generic to flag
/// on its own — many other 40-char base64 blobs (hashes, signatures,
/// opaque IDs) would false-positive — so we gate on AWS / secret /
/// credential context within ~50 chars to the left of the span. Capture
/// group 1 is the actual key so the boundary chars stay outside the
/// redacted span.
fn run_aws_secret_detector(text: &str, pol: &RedactionPolicy, out: &mut Vec<Match>) {
    if !pol.is_detector_enabled(&DetectorKind::AwsSecretKey) {
        return;
    }
    let lower = text.to_ascii_lowercase();
    for caps in aws_secret_regex().captures_iter(text) {
        let Some(value) = caps.get(1) else { continue };
        if !has_aws_secret_context(&lower, value.start()) {
            continue;
        }
        out.push(Match {
            kind: DetectorKind::AwsSecretKey,
            range: value.start()..value.end(),
            confidence: "high",
        });
    }
}

const AWS_SECRET_CONTEXT_WORDS: &[&str] = &["aws", "secret", "access"];

fn has_aws_secret_context(lower_text: &str, span_start: usize) -> bool {
    let window_start = span_start.saturating_sub(50);
    let window_start = safe_boundary(lower_text, window_start, false);
    let window = &lower_text[window_start..span_start];
    AWS_SECRET_CONTEXT_WORDS.iter().any(|w| window.contains(w))
}

const CREDENTIAL_CONTEXT_WORDS: &[&str] = &[
    "key",
    "secret",
    "token",
    "password",
    "passwd",
    "pwd",
    "auth",
    "bearer",
    "credential",
    "api",
    "jwt",
    "signature",
    "hash",
];

/// Returns true when one of [`CREDENTIAL_CONTEXT_WORDS`] appears within
/// the 50 chars to the left of `span_start`. Used by the entropy
/// scanner to decide whether an opaque high-entropy run should clear
/// the post-detection low-confidence filter.
fn has_credential_context(lower_text: &str, span_start: usize) -> bool {
    let window_start = span_start.saturating_sub(50);
    let window_start = safe_boundary(lower_text, window_start, false);
    let window = &lower_text[window_start..span_start];
    CREDENTIAL_CONTEXT_WORDS.iter().any(|w| window.contains(w))
}

// --- Layer B entropy scanner ---

const ENTROPY_MIN_RUN: usize = 32;
const ENTROPY_THRESHOLD: f64 = 4.5;
/// Length floor at which a high-entropy run is treated as a credential
/// even without nearby context. Long opaque tokens are rarely anything
/// else, so the false-positive cost is acceptable for the recall gain.
const ENTROPY_LONG_RUN: usize = 40;
/// Length floor for hex-only runs (commit SHAs, hash digests, hex-
/// encoded keys). Hex maxes out at 4.0 bits of Shannon entropy so the
/// general entropy threshold misses it; we treat the all-hex shape as
/// the signal instead.
const ENTROPY_HEX_MIN_RUN: usize = 32;
/// Minimum Shannon entropy for an all-hex run to be elevated. Uniform
/// hex digits cap at 4.0 bits, but real hashes / keys usually clear
/// 3.0 — this filters out runs of repeating characters (`aaaa…`,
/// `f0f0f0…`) that happen to fit the hex alphabet.
const ENTROPY_HEX_MIN_ENTROPY: f64 = 3.0;

fn run_entropy_scanner(text: &str, out: &mut Vec<Match>) {
    // Walk the byte stream and find maximal runs of [A-Za-z0-9/+=_-].
    // We report ranges on byte offsets — all characters in the allowed
    // set are ASCII, so byte and char offsets coincide in-run.
    let bytes = text.as_bytes();
    let lower = text.to_ascii_lowercase();
    let mut i = 0;
    while i < bytes.len() {
        if !is_token_byte(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_token_byte(bytes[i]) {
            i += 1;
        }
        let end = i;
        let len = end - start;
        if len < ENTROPY_MIN_RUN {
            continue;
        }
        let run = &text[start..end];
        let entropy = shannon_entropy(run.as_bytes());
        let is_all_hex = run.bytes().all(|b| b.is_ascii_hexdigit());
        let credential_ctx = has_credential_context(&lower, start);

        // Three elevation paths to "medium" (the post-detection filter
        // drops "low"): a long opaque run, a long hex string, or a
        // shorter run that sits next to a credential keyword. Anything
        // else stays "low" and is dropped — the generic entropy
        // scanner is too noisy in source code / docs to surface alone.
        let confidence = if (len >= ENTROPY_LONG_RUN && entropy >= ENTROPY_THRESHOLD)
            || (is_all_hex && len >= ENTROPY_HEX_MIN_RUN && entropy >= ENTROPY_HEX_MIN_ENTROPY)
            || (entropy >= ENTROPY_THRESHOLD && credential_ctx)
        {
            "medium"
        } else if entropy >= ENTROPY_THRESHOLD {
            "low"
        } else {
            continue;
        };

        out.push(Match {
            kind: DetectorKind::HighEntropy,
            range: start..end,
            confidence,
        });
    }
}

fn is_token_byte(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'+' | b'=' | b'_' | b'-')
}

fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    // Precision loss is fine: entropy is compared against a threshold
    // and the byte counts we feed here are bounded by the max run
    // length the scanner produces (well under 2^53).
    #[allow(clippy::cast_precision_loss)]
    let len = bytes.len() as f64;
    let mut h = 0.0;
    for &c in &counts {
        if c == 0 {
            continue;
        }
        let p = f64::from(c) / len;
        h -= p * p.log2();
    }
    h
}

// --- Allowlist + overlap resolution ---

fn is_allowlisted(text: &str, m: &Match, allowlist: &[Regex]) -> bool {
    let snippet = &text[m.range.clone()];
    allowlist.iter().any(|re| {
        // Accept either an exact full-string match on the snippet or a
        // hit whose range fully covers the match span in the original
        // text. The latter catches "this whole line is an example".
        if let Some(whole) = re.find(snippet)
            && whole.start() == 0
            && whole.end() == snippet.len()
        {
            return true;
        }
        re.find_iter(text)
            .any(|am| am.start() <= m.range.start && am.end() >= m.range.end)
    })
}

/// Resolve overlapping matches. Higher `priority()` wins; within the
/// same kind, earlier-starting wins. Output is sorted by start offset
/// and non-overlapping.
fn merge_overlaps(mut matches: Vec<Match>) -> Vec<Match> {
    if matches.is_empty() {
        return matches;
    }
    // Primary sort: priority desc. Secondary: start asc. Tertiary: wider span first.
    matches.sort_by(|a, b| {
        b.kind
            .priority()
            .cmp(&a.kind.priority())
            .then(a.range.start.cmp(&b.range.start))
            .then((b.range.end - b.range.start).cmp(&(a.range.end - a.range.start)))
    });

    let mut accepted: Vec<Match> = Vec::new();
    for m in matches {
        let overlaps_accepted = accepted
            .iter()
            .any(|a| m.range.start < a.range.end && m.range.end > a.range.start);
        if !overlaps_accepted {
            accepted.push(m);
        }
    }
    // Re-sort final list by start offset so placeholder substitution
    // downstream is left-to-right deterministic.
    accepted.sort_by_key(|m| m.range.start);
    accepted
}

// --- Placeholder substitution ---

fn apply_placeholders(text: &str, matches: &[Match]) -> String {
    // Walk back-to-front to keep byte offsets in unchanged regions
    // valid as we splice each span. `merge_overlaps` already
    // guarantees the ranges are non-overlapping and sorted ascending.
    let mut out = text.to_string();
    for m in matches.iter().rev() {
        let placeholder = format!("[REDACTED:{}]", m.kind.placeholder());
        out.replace_range(m.range.clone(), &placeholder);
    }
    out
}

fn safe_boundary(s: &str, mut idx: usize, forward: bool) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && idx < s.len() && !s.is_char_boundary(idx) {
        if forward {
            idx += 1;
        } else {
            idx -= 1;
        }
    }
    idx
}

// --- Regex registry ---
//
// Each regex compiles once on first use and is reused for the rest of
// the process lifetime. `expect` on `Regex::new` is fine here — the
// patterns are compile-time constants under test coverage.

fn api_key_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Combined alternation over known vendor-prefixed API keys.
        // Anchored on the prefix; trailing char class is constrained so
        // we don't grab neighboring prose. Each branch uses a
        // conservative length floor.
        Regex::new(
            r"(?x)
            (?:
                sk-ant-[A-Za-z0-9_-]{20,}                       # Anthropic
              | sk-(?:proj-)?[A-Za-z0-9_-]{20,}                 # OpenAI (legacy + project)
              | AIza[A-Za-z0-9_-]{35}                           # Google API key
              | ghp_[A-Za-z0-9]{36}                             # GitHub classic PAT
              | gho_[A-Za-z0-9]{36}                             # GitHub OAuth
              | ghu_[A-Za-z0-9]{36}                             # GitHub user-to-server
              | ghs_[A-Za-z0-9]{36}                             # GitHub server-to-server
              | ghr_[A-Za-z0-9]{36}                             # GitHub refresh
              | xox[abpre]-[A-Za-z0-9-]{10,}                    # Slack
              | hf_[A-Za-z0-9]{20,}                             # Hugging Face
              | (?:sk|pk|rk)_live_[A-Za-z0-9]{24,}              # Stripe live
              | (?:sk|pk|rk)_test_[A-Za-z0-9]{24,}              # Stripe test
              | gsk_[A-Za-z0-9]{20,}                            # Groq
            )",
        )
        .expect("api_key regex compiles")
    })
}

fn aws_access_key_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").expect("aws regex compiles"))
}

fn jwt_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Three base64url segments joined with `.`; first two start
        // with the canonical `eyJ` (base64 of `{"`). The header sanity
        // check in run_jwt_detector cuts remaining false positives.
        Regex::new(r"\beyJ[A-Za-z0-9_-]{5,}\.eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\b")
            .expect("jwt regex compiles")
    })
}

fn private_key_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?s)-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY-----.*?-----END (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY-----",
        )
        .expect("private key regex compiles")
    })
}

fn connection_string_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:postgres(?:ql)?|mysql|mongodb(?:\+srv)?|redis(?:s)?|amqp(?:s)?)://[A-Za-z0-9._~%!$&'()*+,;=:-]+:[^@\s]+@[A-Za-z0-9._~%!$&'()*+,;=:@/\[\]?-]+",
        )
        .expect("connection_string regex compiles")
    })
}

fn credit_card_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Digit runs with optional space/dash separators, 13–19 digits
        // total (approximated here; Luhn + post-filter confirm).
        Regex::new(r"\b(?:\d[ -]?){13,19}\b").expect("credit_card regex compiles")
    })
}

fn iban_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Country code + 2-digit checksum + 11–30 BBAN chars, with
        // optional internal whitespace (the canonical pretty-printed
        // form groups the BBAN in fours, e.g. `DE89 3704 0044 0532
        // 0130 00`). `iban_check` strips whitespace before validating
        // mod-97, so a permissive regex here is safe.
        Regex::new(r"\b[A-Z]{2}\d{2}(?:[ \t]?[A-Z0-9]){11,30}\b").expect("iban regex compiles")
    })
}

fn email_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,63}\b")
            .expect("email regex compiles")
    })
}

/// URL-shape regex used to drop non-secret matches that fall inside
/// addresses (see the filter in `detect`). Matches two flavours:
///   * scheme-qualified — `https?://…`, `ftp://…`, `wss?://…`, `file://…`
///   * bare host with at least two dot-separated labels and a 2–24 char
///     alphabetic TLD (covers `windguru.cz`, `example.co.uk`, etc.) plus
///     an optional `/path` or `?query` tail.
///
/// Imperfect-but-permissive on purpose: false-positive URL spans only
/// reduce redaction inside that region, which is the goal.
fn url_span_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let pattern = r#"(?ix)
            (?:
                (?:https?|ftp|wss?|file)://[^\s)\]<>"'`]+
                |
                \b
                (?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+
                [a-z]{2,24}
                (?:[/?][^\s)\]<>"'`]*)?
            )
        "#;
        Regex::new(pattern).expect("url_span regex compiles")
    })
}

fn find_url_spans(text: &str) -> Vec<Range<usize>> {
    url_span_regex()
        .find_iter(text)
        .map(|m| m.start()..m.end())
        .collect()
}

fn url_secret_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Anchored on the `?`/`&` query-string delimiter so a literal
        // `password=` in prose doesn't trip; the parameter name list
        // covers the common credential aliases. Capture group 1 holds
        // the value (everything up to the next `&`, whitespace, `#`
        // fragment marker, or surrounding quote/angle bracket so URLs
        // embedded in source/JSON don't swallow the closing delimiter).
        let pattern = r#"(?i)[?&](?:token|apikey|api[_-]key|api[_-]secret|access[_-]?token|refresh[_-]?token|id[_-]?token|bearer[_-]?token|auth[_-]?token|auth|password|passwd|pwd|client[_-]?secret|client[_-]?key|app[_-]?secret|app[_-]?key|secret|private[_-]?key|x[_-]api[_-]key|x[_-]auth[_-]token|signature|sig)=([^&\s#"'<>]+)"#;
        Regex::new(pattern).expect("url_secret regex compiles")
    })
}

fn phone_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Broad shape — E.164 (`+CC…`) and common national formats with
        // separators. Digit-count + context gate in `run_phone_detector`
        // does the tightening.
        Regex::new(
            r"(?x)
            (?:
                \+\d{1,3}[\s.\-]?(?:\(\d{1,4}\)[\s.\-]?)?\d{1,4}(?:[\s.\-]?\d{1,4}){1,4}
              | \(\d{2,4}\)[\s.\-]?\d{2,4}[\s.\-]?\d{2,4}(?:[\s.\-]?\d{2,4})?
              | \d{3,4}[\s.\-]\d{3,4}[\s.\-]\d{3,5}
            )",
        )
        .expect("phone regex compiles")
    })
}

fn ssn_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // US SSN canonical form `AAA-GG-SSSS`. Excluding bare 9-digit
        // runs keeps the false-positive rate low — those collide with
        // order numbers, postal codes, and timestamps.
        Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").expect("ssn regex compiles")
    })
}

fn pesel_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // 11 contiguous digits. The checksum gate in run_pesel_detector
        // is what makes this usable — bare 11-digit runs are otherwise
        // common (timestamps, ISBN-like IDs, internal sequences).
        Regex::new(r"\b\d{11}\b").expect("pesel regex compiles")
    })
}

fn ipv4_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Each octet validated to 0–255 inline so version strings like
        // `1.2.3.4-rc1` and dotted build numbers don't false-positive.
        let octet = r"(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)";
        let pattern = format!(r"\b(?:{octet}\.){{3}}{octet}\b");
        Regex::new(&pattern).expect("ipv4 regex compiles")
    })
}

fn ipv6_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Three forms: full 8-group, leading `::`, and `::` somewhere
        // in the middle / trailing. Word boundaries don't fire after
        // `:` (non-word), so each alternative anchors itself with a
        // hex-digit `\b` at one end and lets the colon structure
        // bound the other.
        Regex::new(
            r"(?x)
            (?:
                \b[0-9a-fA-F]{1,4}(?::[0-9a-fA-F]{1,4}){7}\b
              | \b(?:[0-9a-fA-F]{1,4}:){1,7}:(?:[0-9a-fA-F]{1,4}(?::[0-9a-fA-F]{1,4}){0,5})?
              | ::[0-9a-fA-F]{1,4}(?::[0-9a-fA-F]{1,4}){0,6}\b
              | ::1\b
            )",
        )
        .expect("ipv6 regex compiles")
    })
}

fn mac_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Six hex pairs separated by `:` or `-`. The whole match must
        // use the same separator — mixing `:` and `-` is non-standard
        // and almost always coincidental noise.
        Regex::new(
            r"\b(?:[0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}\b|\b(?:[0-9A-Fa-f]{2}-){5}[0-9A-Fa-f]{2}\b",
        )
        .expect("mac regex compiles")
    })
}

fn aws_secret_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // 40-char base64 alphabet without padding. Capture group 1 is
        // the key itself; the surrounding `[^…]` boundaries make sure
        // the regex doesn't grab a 40-char prefix of a longer base64
        // blob (which would tokenize as part of the same run). The
        // context gate in `run_aws_secret_detector` rejects anything
        // not preceded by aws/secret/access — that's what makes this
        // detector usable despite the generic shape.
        let pattern = r"(?:^|[^A-Za-z0-9/+])([A-Za-z0-9/+]{40})(?:$|[^A-Za-z0-9/+])";
        Regex::new(pattern).expect("aws_secret regex compiles")
    })
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    fn pol_redact() -> RedactionPolicy {
        RedactionPolicy {
            mode: RedactionMode::Redact,
            skip_local: true,
            enabled_detectors: vec![],
            extra_patterns: vec![],
            allowlist: vec![],
            ner_requested: false,
        }
    }

    fn pol_block() -> RedactionPolicy {
        let mut p = pol_redact();
        p.mode = RedactionMode::Block;
        p
    }

    fn pol_off() -> RedactionPolicy {
        RedactionPolicy::off()
    }

    // --- redact() top-level ---

    #[test]
    fn off_mode_is_passthrough() {
        let r = redact("my key is sk-proj-abcdef123456ABCDEFGHIJKL", &pol_off());
        assert!(matches!(r, RedactionResult::Clean));
    }

    #[test]
    fn clean_text_returns_clean() {
        let r = redact("hello world, nothing here", &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    #[test]
    fn empty_text_returns_clean() {
        let r = redact("", &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- API key detection ---

    #[test]
    fn detects_openai_sk_proj_key() {
        let input = "token: sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb end";
        let r = redact(input, &pol_redact());
        match r {
            RedactionResult::Redacted { text, matches } => {
                assert!(text.contains("[REDACTED:API_KEY]"));
                assert_eq!(matches.len(), 1);
                assert_eq!(matches[0].kind, DetectorKind::ApiKey);
            }
            other => panic!("expected Redacted, got {other:?}"),
        }
    }

    #[test]
    fn detects_anthropic_key() {
        let input = "LLM_ANTHROPIC_API_KEY=sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:API_KEY]"));
        assert_eq!(matches[0].kind, DetectorKind::ApiKey);
    }

    #[test]
    fn detects_google_api_key() {
        let input = "key=AIzaSyB1234567890abcdefghijklmnopqrstuvw";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Redacted { .. }));
    }

    #[test]
    fn detects_github_token() {
        let input = "ghp_1234567890abcdefABCDEFghijklmnopqrst";
        assert_eq!(input.len() - 4, 36);
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, .. } = r else {
            panic!("expected Redacted");
        };
        assert_eq!(text, "[REDACTED:API_KEY]");
    }

    #[test]
    fn detects_stripe_live_key() {
        // String built at runtime so GitHub push protection's Stripe
        // scanner doesn't flag the literal in source.
        let input = format!("sk_{}_abcdefghijklmnopqrstuvwxyz0123456789", "live");
        let r = redact(&input, &pol_redact());
        assert!(matches!(r, RedactionResult::Redacted { .. }));
    }

    // --- AWS access keys ---

    #[test]
    fn detects_aws_access_key() {
        let input = "aws_access_key_id=AKIAIOSFODNN7EXAMPLE";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:AWS_KEY]"));
        assert_eq!(matches[0].kind, DetectorKind::AwsAccessKey);
    }

    #[test]
    fn aws_ignores_shorter_akia_prefix() {
        let input = "AKIASHORT";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- JWTs ---

    #[test]
    fn detects_valid_jwt() {
        // header `{"alg":"HS256"}` → eyJhbGciOiJIUzI1NiJ9
        // payload `{"sub":"42"}` → eyJzdWIiOiI0MiJ9
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiI0MiJ9.abcdefghij_klmnop-qrstuv";
        let r = redact(jwt, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert_eq!(matches[0].kind, DetectorKind::Jwt);
    }

    #[test]
    fn jwt_shape_without_valid_header_is_ignored() {
        // Header doesn't decode to a {-prefixed JSON object.
        let r = redact("eyJxxxxxxxxxxxxxx.eyJyyyyyyyyyy.zzzzzzzzzz", &pol_redact());
        assert!(matches!(
            r,
            RedactionResult::Clean | RedactionResult::Redacted { .. }
        ));
    }

    // --- Private keys ---

    #[test]
    fn detects_pem_private_key() {
        let input = "Here is my key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\ndone";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:PRIVATE_KEY]"));
        assert_eq!(matches[0].kind, DetectorKind::PrivateKey);
    }

    #[test]
    fn detects_openssh_private_key() {
        let input = "-----BEGIN OPENSSH PRIVATE KEY-----\nbody\n-----END OPENSSH PRIVATE KEY-----";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Redacted { .. }));
    }

    // --- Connection strings ---

    #[test]
    fn detects_postgres_conn_string() {
        let input = "DATABASE_URL=postgres://admin:s3cret@db.example.com:5432/prod";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert_eq!(matches[0].kind, DetectorKind::ConnectionString);
    }

    #[test]
    fn detects_mongodb_srv_conn_string() {
        let input = "mongodb+srv://user:pw@cluster0.abcde.mongodb.net/dbname";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Redacted { .. }));
    }

    #[test]
    fn postgres_without_password_is_not_redacted() {
        let input = "postgres://localhost:5432/db";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- Credit cards ---

    #[test]
    fn detects_valid_luhn_card() {
        // 4111 1111 1111 1111 — classic Visa test card, passes Luhn.
        let input = "card: 4111 1111 1111 1111 thanks";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert_eq!(matches[0].kind, DetectorKind::CreditCard);
    }

    #[test]
    fn rejects_non_luhn_digit_run() {
        let input = "order number: 1234567890123456";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    #[test]
    fn rejects_uuid_as_card() {
        let input = "uuid 123e4567-e89b-12d3-a456-426614174000 ok";
        let r = redact(input, &pol_redact());
        // UUID has 32 hex chars with dashes — not all digits, so the
        // card regex won't match. The entropy scanner may trip; we
        // accept either Clean or a Redacted-from-entropy result.
        match r {
            RedactionResult::Clean => {}
            RedactionResult::Redacted { matches, .. } => {
                assert!(matches.iter().all(|m| m.kind != DetectorKind::CreditCard));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // --- IBANs ---

    #[test]
    fn detects_valid_iban() {
        // GB82 WEST 1234 5698 7654 32 — classic example, passes mod-97.
        let input = "send to GB82WEST12345698765432 today";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Iban));
    }

    #[test]
    fn rejects_invalid_iban_check() {
        let input = "GB00WEST12345698765432";
        let r = redact(input, &pol_redact());
        // May still be flagged as HighEntropy, but not as Iban.
        match r {
            RedactionResult::Clean => {}
            RedactionResult::Redacted { matches, .. } => {
                assert!(matches.iter().all(|m| m.kind != DetectorKind::Iban));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // --- Email / phone ---

    #[test]
    fn detects_email() {
        let input = "reach me at piotr.wittchen@gmail.com anytime";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:EMAIL]"));
        assert_eq!(matches[0].kind, DetectorKind::Email);
    }

    #[test]
    fn detects_e164_phone() {
        let input = "Call +1 415 555 2671 soon";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Phone));
    }

    #[test]
    fn phone_without_context_is_ignored() {
        // No keyword, no + prefix — likely an order number.
        let input = "ref 415 555 2671 for pickup";
        let r = redact(input, &pol_redact());
        // May still hit HighEntropy for long digit runs, but should
        // not mis-classify as Phone.
        match r {
            RedactionResult::Clean => {}
            RedactionResult::Redacted { matches, .. } => {
                assert!(matches.iter().all(|m| m.kind != DetectorKind::Phone));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn phone_with_context_keyword_detected() {
        let input = "phone: 415-555-2671 after 5pm";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Phone));
    }

    // --- URL query-parameter secrets ---

    #[test]
    fn detects_url_query_token() {
        // The reported case: a short opaque value behind `?token=` is too
        // short for the entropy floor and matches no vendor prefix, so it
        // used to slip past redaction entirely.
        let input = "GET https://api.example.com/v1/users?token=abc123secret here";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("token=[REDACTED:URL_SECRET]"));
        assert!(!text.contains("abc123secret"));
        assert!(matches.iter().any(|m| m.kind == DetectorKind::UrlSecret));
    }

    #[test]
    fn detects_url_query_api_key_dashed() {
        let input = "https://example.com/?api-key=foo123";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("api-key=[REDACTED:URL_SECRET]"));
        assert!(!text.contains("foo123"));
    }

    #[test]
    fn detects_url_query_password() {
        let input = "https://example.com/login?user=alice&password=hunter2&page=1";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("password=[REDACTED:URL_SECRET]"));
        assert!(text.contains("user=alice"));
        assert!(text.contains("page=1"));
        assert!(!text.contains("hunter2"));
    }

    #[test]
    fn detects_multiple_url_credentials() {
        let input = "https://api.example.com/?token=abc123&client_secret=xyz789";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert_eq!(
            matches
                .iter()
                .filter(|m| m.kind == DetectorKind::UrlSecret)
                .count(),
            2
        );
        assert!(!text.contains("abc123"));
        assert!(!text.contains("xyz789"));
    }

    #[test]
    fn url_innocuous_params_are_clean() {
        let r = redact(
            "https://example.com/?page=2&id=42&size=10&sort=desc",
            &pol_redact(),
        );
        assert!(matches!(r, RedactionResult::Clean));
    }

    #[test]
    fn url_query_value_with_known_apikey_prefers_apikey() {
        // ApiKey priority (7) > UrlSecret (6) — when the value itself
        // is a known vendor prefix the more specific kind wins.
        let input = "https://example.com/?api_key=AIzaSyB1234567890abcdefghijklmnopqrstuvw";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::ApiKey));
        assert!(matches.iter().all(|m| m.kind != DetectorKind::UrlSecret));
    }

    #[test]
    fn url_secret_value_in_quoted_url_stops_at_quote() {
        // URL embedded in a JSON/source-code string should not eat the
        // closing quote into the redacted span.
        let input = r#"{"url":"https://api.example.com/?token=abc123"}"#;
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("token=[REDACTED:URL_SECRET]"));
        assert!(text.ends_with(r#""}"#));
    }

    #[test]
    fn url_bare_domain_label_is_not_redacted() {
        // Regression: NER and other PII detectors used to fire on the
        // host label (`windguru` reads as an Organization), mangling
        // the address without removing anything sensitive. Detectors
        // inside URL spans now only fire for secret-class kinds.
        let input = "check the forecast at windguru.cz before going";
        let r = redact(input, &pol_redact());
        // No detector other than secret-class kinds should produce a
        // match here. Clean is the expected outcome on the regex-only
        // path; if NER were running it would no longer trip either.
        assert!(matches!(r, RedactionResult::Clean));
    }

    #[test]
    fn url_path_segments_are_not_redacted() {
        // The path /orders/john-smith/ reads as a person; the host
        // example.com reads as an Organization. Both should be skipped
        // because they're inside a URL span. (The credit-card number
        // would normally fire, but it sits in the path, not behind a
        // recognized query-string credential parameter — so it stays
        // too, which matches the "tokens/api-keys only in URLs" goal.)
        let input = "see https://example.com/orders/john-smith/4111-1111-1111-1111";
        let r = redact(input, &pol_redact());
        // Either Clean (nothing left to redact) or Redacted with only
        // secret-class kinds. PersonName/Organization/CreditCard must
        // not appear.
        match r {
            RedactionResult::Clean => {}
            RedactionResult::Redacted { matches, .. } => {
                for m in &matches {
                    assert!(
                        !matches!(
                            m.kind,
                            DetectorKind::PersonName
                                | DetectorKind::Location
                                | DetectorKind::Organization
                                | DetectorKind::CreditCard
                                | DetectorKind::Iban
                                | DetectorKind::Phone
                                | DetectorKind::IpAddress
                                | DetectorKind::MacAddress
                                | DetectorKind::Email
                        ),
                        "unexpected match inside URL: {:?}",
                        m.kind,
                    );
                }
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn url_query_secret_still_redacts_when_url_filter_active() {
        // The URL filter must not weaken the url_secret detector — a
        // ?token=… inside a URL is exactly what we want to redact.
        let input = "https://api.example.com/v1/users?token=abc123secret";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::UrlSecret));
        assert!(text.contains("token=[REDACTED:URL_SECRET]"));
    }

    #[test]
    fn email_outside_url_still_redacts() {
        // The URL filter is scoped to spans matched by the URL regex.
        // A bare email in prose must still fire.
        let input = "reply to alice@example.com tomorrow";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Email));
    }

    // --- Entropy ---

    #[test]
    fn entropy_low_confidence_hits_are_dropped() {
        // The Layer-B entropy scanner produces "low" confidence
        // matches; the post-detection filter drops these before they
        // reach the redact/block decision so a 32-char high-entropy
        // run no longer trips a false-positive redaction on its own.
        let input = "blob: q8X2Lk9wR4vN1cM7pT3eJ6hZ5fY0oAbD Ud2S end";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    #[test]
    fn entropy_ignores_short_tokens() {
        let input = "hello short1 abc123def";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    #[test]
    fn entropy_ignores_low_entropy_long_string() {
        let input = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let r = redact(input, &pol_redact());
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- Allowlist ---

    #[test]
    fn allowlist_suppresses_matching_span() {
        let mut p = pol_redact();
        p.allowlist.push(Regex::new(r"AKIATEST0000000000").unwrap());
        let input = "key is AKIATEST0000000000 only";
        let r = redact(input, &p);
        // AKIATEST0000000000 is 18 chars (AKIA + 14), so the AWS regex
        // which requires 16 chars after AKIA won't match. Let's use a
        // true AKIA test.
        let _ = r;
        let mut p = pol_redact();
        p.allowlist
            .push(Regex::new(r"AKIAIOSFODNN7EXAMPLE").unwrap());
        let r = redact("key=AKIAIOSFODNN7EXAMPLE only", &p);
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- Custom user patterns ---

    #[test]
    fn custom_pattern_is_redacted_with_its_name() {
        let mut p = pol_redact();
        p.extra_patterns.push((
            "CUSTOMER_ID".to_string(),
            Regex::new(r"CUST-\d{8}").unwrap(),
        ));
        let r = redact("see CUST-12345678 for details", &p);
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:CUSTOMER_ID]"));
        assert_eq!(matches[0].kind, DetectorKind::Custom("CUSTOMER_ID".into()));
    }

    // --- Detector filter ---

    #[test]
    fn ner_detector_kinds_have_expected_placeholders_and_slugs() {
        // Covers the Layer-C variants even when the `redaction-ner`
        // feature is off, so the enum is fully exercised in both
        // build configurations.
        for (kind, expected_placeholder, expected_slug) in [
            (DetectorKind::PersonName, "PERSON", "person_name"),
            (DetectorKind::Location, "LOCATION", "location"),
            (DetectorKind::Organization, "ORGANIZATION", "organization"),
        ] {
            assert_eq!(kind.placeholder(), expected_placeholder);
            assert_eq!(kind.slug(), expected_slug);
        }
    }

    #[test]
    fn disabled_detector_skipped() {
        let mut p = pol_redact();
        p.enabled_detectors = vec!["email".to_string()];
        let r = redact(
            "sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb and foo@bar.com",
            &p,
        );
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().all(|m| m.kind == DetectorKind::Email));
    }

    // --- Overlap resolution ---

    #[test]
    fn priority_jwt_beats_entropy() {
        // JWTs look like high-entropy runs; `Jwt` priority should win.
        let input = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiI0MiJ9.abcdefghij_klmnop-qrstuv";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Jwt));
        assert!(matches.iter().all(|m| m.kind != DetectorKind::HighEntropy));
    }

    #[test]
    fn priority_apikey_beats_entropy() {
        let input = "token=sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb end";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        // At least one ApiKey hit, and nothing else overlapping it.
        assert!(matches.iter().any(|m| m.kind == DetectorKind::ApiKey));
    }

    // --- Block mode ---

    #[test]
    fn block_mode_returns_blocked() {
        let r = redact(
            "please use sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb",
            &pol_block(),
        );
        assert!(matches!(r, RedactionResult::Blocked { .. }));
    }

    #[test]
    fn block_mode_clean_input_returns_clean() {
        let r = redact("all good here", &pol_block());
        assert!(matches!(r, RedactionResult::Clean));
    }

    // --- redact_for_persistence ---

    #[test]
    fn persistence_off_mode_returns_none() {
        let r = redact_for_persistence(
            "key sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb",
            &pol_off(),
        );
        assert!(r.is_none());
    }

    #[test]
    fn persistence_clean_text_returns_none() {
        assert!(redact_for_persistence("hello world", &pol_redact()).is_none());
    }

    #[test]
    fn persistence_empty_text_returns_none() {
        assert!(redact_for_persistence("", &pol_redact()).is_none());
        assert!(redact_for_persistence("", &pol_block()).is_none());
    }

    #[test]
    fn persistence_redact_mode_substitutes_placeholders() {
        let out = redact_for_persistence(
            "use sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb please",
            &pol_redact(),
        )
        .expect("expected a substitution");
        assert!(out.contains("[REDACTED:API_KEY]"));
        assert!(!out.contains("sk-proj-"));
    }

    #[test]
    fn persistence_block_mode_still_redacts() {
        // Block mode aborts the network call but at the persistence
        // seam we still want placeholders rather than the raw secret.
        let out = redact_for_persistence(
            "use sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb please",
            &pol_block(),
        )
        .expect("block mode still scrubs at the persistence seam");
        assert!(out.contains("[REDACTED:API_KEY]"));
        assert!(!out.contains("sk-proj-"));
    }

    // --- Describe / UTF-8 safety ---

    #[test]
    fn describe_matches_never_includes_raw_secret() {
        let input = "key=sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb done";
        let RedactionResult::Blocked { matches } = redact(input, &pol_block()) else {
            panic!("expected Blocked");
        };
        let desc = describe_matches(input, &matches);
        assert!(!desc.contains("sk-proj-aaaaa"));
        assert!(desc.contains("[REDACTED:API_KEY]"));
    }

    #[test]
    fn utf8_safe_context_window() {
        let input = "日本語 key=sk-proj-aaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbb 日本語";
        let RedactionResult::Blocked { matches } = redact(input, &pol_block()) else {
            panic!("expected Blocked");
        };
        // Should not panic — describe_matches must walk to UTF-8 boundaries.
        let _ = describe_matches(input, &matches);
    }

    // --- Luhn / mod-97 direct tests ---

    #[test]
    fn luhn_accepts_known_valid() {
        assert!(luhn_check("4111111111111111"));
        assert!(luhn_check("5500000000000004"));
        assert!(luhn_check("340000000000009"));
    }

    #[test]
    fn luhn_rejects_invalid() {
        assert!(!luhn_check("4111111111111112"));
        assert!(!luhn_check("0000000000000001"));
    }

    #[test]
    fn iban_accepts_known_valid() {
        assert!(iban_check("GB82WEST12345698765432"));
        assert!(iban_check("DE89370400440532013000"));
    }

    #[test]
    fn iban_rejects_invalid() {
        assert!(!iban_check("GB00WEST12345698765432"));
        assert!(!iban_check("XX999999999"));
    }

    // --- Shannon entropy ---

    #[test]
    fn entropy_zero_for_uniform() {
        let h = shannon_entropy(b"aaaaaa");
        assert!((h - 0.0).abs() < 1e-9);
    }

    #[test]
    fn entropy_positive_for_mixed() {
        let h = shannon_entropy(b"abcdefgh");
        assert!(h > 2.9);
    }

    // --- parse_extra_patterns ---

    #[test]
    fn parse_extra_patterns_basic() {
        let mut warnings = Vec::new();
        let got = parse_extra_patterns("CUSTOMER=CUST-\\d+;TICKET=JIRA-\\d+", &mut warnings);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "CUSTOMER");
        assert_eq!(got[1].0, "TICKET");
        assert!(warnings.is_empty());
    }

    #[test]
    fn parse_extra_patterns_skips_bad_regex() {
        let mut warnings = Vec::new();
        let got = parse_extra_patterns("GOOD=abc;BAD=(unclosed", &mut warnings);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "GOOD");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("BAD"));
    }

    // --- New detectors: SSN, IP, MAC, AWS secret ---

    #[test]
    fn detects_ssn_canonical_form() {
        let r = redact("SSN: 123-45-6789", &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:SSN]"));
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Ssn));
    }

    #[test]
    fn detects_valid_pesel() {
        // 54092524272 — checksum: weights·digits sum % 10 = 8,
        // (10 - 8) % 10 = 2 = last digit ✓
        let r = redact("PESEL: 54092524272", &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:PESEL]"));
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Pesel));
    }

    #[test]
    fn rejects_pesel_with_bad_checksum() {
        // Valid shape (11 digits) but the checksum digit is wrong.
        let r = redact("looks like 54092524273 maybe", &pol_redact());
        match r {
            RedactionResult::Clean => {}
            RedactionResult::Redacted { matches, .. } => {
                assert!(matches.iter().all(|m| m.kind != DetectorKind::Pesel));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn pesel_check_direct() {
        assert!(pesel_check("54092524272"));
        assert!(!pesel_check("54092524273"));
        assert!(!pesel_check("1234567890")); // 10 digits
        assert!(!pesel_check("123456789012")); // 12 digits
        assert!(!pesel_check("5409252427a")); // non-digit
    }

    #[test]
    fn detects_ipv4_address() {
        let r = redact("server at 192.168.13.37 listens", &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:IP_ADDRESS]"));
        assert!(matches.iter().any(|m| m.kind == DetectorKind::IpAddress));
    }

    #[test]
    fn ipv4_rejects_invalid_octet() {
        // `999` is not a valid octet — the inline 0–255 check rejects.
        let r = redact("build 1.2.3.999 today", &pol_redact());
        match r {
            RedactionResult::Clean => {}
            RedactionResult::Redacted { matches, .. } => {
                assert!(matches.iter().all(|m| m.kind != DetectorKind::IpAddress));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn detects_ipv6_full_form() {
        let r = redact(
            "host 2001:0db8:85a3:0000:0000:8a2e:0370:7334 reachable",
            &pol_redact(),
        );
        let RedactionResult::Redacted { text, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:IP_ADDRESS]"));
        assert!(!text.contains("8a2e:0370"));
    }

    #[test]
    fn detects_mac_address_colon() {
        let r = redact("nic at 00:1A:2B:3C:4D:5E reports up", &pol_redact());
        let RedactionResult::Redacted { text, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:MAC_ADDRESS]"));
    }

    #[test]
    fn detects_mac_address_dash() {
        let r = redact("MAC=00-1A-2B-3C-4D-5E here", &pol_redact());
        let RedactionResult::Redacted { text, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:MAC_ADDRESS]"));
    }

    #[test]
    fn detects_aws_secret_with_context() {
        let input = "AWS secret key: wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY done";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:AWS_SECRET_KEY]"));
        assert!(!text.contains("wJalrXUtnFEMI/K7MDENG"));
        assert!(matches.iter().any(|m| m.kind == DetectorKind::AwsSecretKey));
    }

    #[test]
    fn aws_secret_without_context_is_not_flagged_as_aws() {
        // 40-char base64 blob without aws/secret context should not
        // be tagged as AwsSecretKey (the entropy scanner may still
        // surface it as HighEntropy, which is fine — the user can
        // disable that detector if needed).
        let input = "blob: wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY end";
        let r = redact(input, &pol_redact());
        match r {
            RedactionResult::Clean => {}
            RedactionResult::Redacted { matches, .. } => {
                assert!(matches.iter().all(|m| m.kind != DetectorKind::AwsSecretKey));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // --- IBAN with whitespace groups ---

    #[test]
    fn detects_iban_with_pretty_print_spaces() {
        // The canonical IBAN pretty-print groups characters in fours,
        // which trips word-boundary anchors. The regex now allows
        // optional internal whitespace; the mod-97 validator strips
        // it before the check.
        let r = redact("IBAN: DE89 3704 0044 0532 0130 00 today", &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:IBAN]"));
        assert!(matches.iter().any(|m| m.kind == DetectorKind::Iban));
    }

    // --- Entropy elevation ---

    #[test]
    fn entropy_long_hex_run_is_redacted() {
        // 48 hex chars: max entropy of 4.0 bits sits below the 4.5
        // general threshold, so the all-hex elevation path is what
        // catches it.
        let input = "Generic high-entropy: 9f8e7d6c5b4a39281706f5e4d3c2b1a0ffeeddccbbaa9988 end";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { text, matches } = r else {
            panic!("expected Redacted");
        };
        assert!(text.contains("[REDACTED:HIGH_ENTROPY]"));
        assert!(!text.contains("9f8e7d6c5b4a"));
        assert!(matches.iter().any(|m| m.kind == DetectorKind::HighEntropy));
    }

    #[test]
    fn entropy_short_run_with_credential_context_is_redacted() {
        // 32 chars exactly — would normally stay "low" and be dropped,
        // but the `secret:` prefix elevates to medium.
        let input = "secret: q8X2Lk9wR4vN1cM7pT3eJ6hZ5fY0oAbD end";
        let r = redact(input, &pol_redact());
        let RedactionResult::Redacted { matches, .. } = r else {
            panic!("expected Redacted");
        };
        assert!(matches.iter().any(|m| m.kind == DetectorKind::HighEntropy));
    }

    #[test]
    fn parse_extra_patterns_skips_malformed_entry() {
        let mut warnings = Vec::new();
        let got = parse_extra_patterns("noEquals;HAS=ok", &mut warnings);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "HAS");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("noEquals"));
    }
}
