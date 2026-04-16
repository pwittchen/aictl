//! Test whether a host:port accepts TCP connections.
//!
//! Pure tokio — no shell, no external binary. Uses `TcpStream::connect`
//! wrapped in `tokio::time::timeout` so unreachable targets return
//! promptly instead of hanging. The tool only completes the TCP
//! three-way handshake; it never sends application data, so it doesn't
//! need a protocol-specific probe.
//!
//! Input format (one line):
//!
//! ```text
//! <host>:<port> [timeout=<ms>]
//! ```
//!
//! Examples:
//!
//! ```text
//! localhost:8080
//! example.com:443 timeout=1000
//! [::1]:8080
//! https://example.com  (scheme stripped, port inferred: 443)
//! ```

use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;

const DEFAULT_TIMEOUT_MS: u64 = 3000;
const MAX_TIMEOUT_MS: u64 = 30_000;

pub(super) async fn tool_check_port(input: &str) -> String {
    let parsed = match parse_input(input) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };
    let timeout = Duration::from_millis(parsed.timeout_ms);
    probe(&parsed.host, parsed.port, timeout).await
}

// ----- parsing -----

#[derive(Debug, PartialEq)]
struct Parsed {
    host: String,
    port: u16,
    timeout_ms: u64,
}

fn parse_input(input: &str) -> Result<Parsed, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("empty input. Expected: <host>:<port> [timeout=<ms>]".to_string());
    }
    if input.contains('\0') {
        return Err("input contains null byte".to_string());
    }

    let mut timeout_ms = DEFAULT_TIMEOUT_MS;
    let mut target: Option<&str> = None;

    for token in input.split_whitespace() {
        if let Some(rest) = token.strip_prefix("timeout=") {
            let ms = rest
                .parse::<u64>()
                .map_err(|e| format!("invalid timeout '{rest}': {e}"))?;
            if ms == 0 {
                return Err("timeout must be > 0".to_string());
            }
            if ms > MAX_TIMEOUT_MS {
                return Err(format!(
                    "timeout {ms}ms exceeds maximum of {MAX_TIMEOUT_MS}ms"
                ));
            }
            timeout_ms = ms;
        } else if target.is_none() {
            target = Some(token);
        } else {
            return Err(format!(
                "unexpected token '{token}'. Expected: <host>:<port> [timeout=<ms>]"
            ));
        }
    }

    let target = target.ok_or_else(|| "missing <host>:<port>".to_string())?;
    let (host, port) = split_host_port(target)?;

    Ok(Parsed {
        host,
        port,
        timeout_ms,
    })
}

fn split_host_port(s: &str) -> Result<(String, u16), String> {
    // Strip common URL schemes so `https://example.com` / `tcp://host:22`
    // parse too. The inferred port for http/https is set when the string
    // has no explicit `:port`.
    let (rest, default_port) = if let Some(r) = s.strip_prefix("https://") {
        (r.trim_end_matches('/'), Some(443u16))
    } else if let Some(r) = s.strip_prefix("http://") {
        (r.trim_end_matches('/'), Some(80u16))
    } else if let Some(r) = s.strip_prefix("tcp://") {
        (r.trim_end_matches('/'), None)
    } else {
        (s, None)
    };

    // Strip a trailing path if the caller passed a full URL (e.g. `example.com/foo`)
    let rest = rest.split('/').next().unwrap_or(rest);

    if rest.is_empty() {
        return Err("empty host".to_string());
    }

    // IPv6 bracketed form: `[::1]:8080` or `[::1]` with inferred port.
    if let Some(inner) = rest.strip_prefix('[') {
        let close = inner
            .find(']')
            .ok_or_else(|| format!("unterminated IPv6 bracket in '{rest}'"))?;
        let host = &inner[..close];
        if host.is_empty() {
            return Err("empty IPv6 host".to_string());
        }
        let tail = &inner[close + 1..];
        let port = if let Some(port_s) = tail.strip_prefix(':') {
            parse_port(port_s)?
        } else if tail.is_empty() {
            default_port.ok_or_else(|| format!("missing port in '{rest}'"))?
        } else {
            return Err(format!("unexpected text after ']': '{tail}'"));
        };
        return Ok((host.to_string(), port));
    }

    // Bare host with optional `:port`. Use `rfind` so the final colon is
    // treated as the host/port separator even for addresses written
    // without brackets (rare but friendlier to errors).
    if let Some(idx) = rest.rfind(':') {
        let host = &rest[..idx];
        let port_s = &rest[idx + 1..];
        if host.is_empty() {
            return Err("empty host".to_string());
        }
        let port = parse_port(port_s)?;
        return Ok((host.to_string(), port));
    }

    let port = default_port.ok_or_else(|| {
        format!("missing port in '{rest}' — expected '<host>:<port>' or an http/https URL")
    })?;
    Ok((rest.to_string(), port))
}

fn parse_port(s: &str) -> Result<u16, String> {
    let n: u32 = s.parse().map_err(|e| format!("invalid port '{s}': {e}"))?;
    if !(1..=65535).contains(&n) {
        return Err(format!("port {n} out of range 1..=65535"));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(n as u16)
}

// ----- probe -----

async fn probe(host: &str, port: u16, timeout: Duration) -> String {
    let target = format!("{host}:{port}");

    // DNS resolution runs on a blocking thread so it doesn't stall the
    // runtime; if it fails the tool reports a clear DNS-level error
    // instead of the generic "connection failed" that `TcpStream::connect`
    // would produce.
    let target_for_dns = target.clone();
    let addrs = match tokio::task::spawn_blocking(move || {
        target_for_dns
            .to_socket_addrs()
            .map(Iterator::collect::<Vec<_>>)
    })
    .await
    {
        Ok(Ok(addrs)) => addrs,
        Ok(Err(e)) => return format!("Unreachable — DNS resolution failed for '{host}': {e}"),
        Err(e) => return format!("Error: DNS lookup task failed: {e}"),
    };

    if addrs.is_empty() {
        return format!("Unreachable — DNS returned no addresses for '{host}'");
    }

    let start = Instant::now();
    let connect = TcpStream::connect(&*addrs);
    match tokio::time::timeout(timeout, connect).await {
        Ok(Ok(stream)) => {
            #[allow(clippy::cast_possible_truncation)]
            let ms = start.elapsed().as_millis() as u64;
            let resolved = stream
                .peer_addr()
                .map_or_else(|_| target.clone(), |a| a.to_string());
            format!("Reachable — {target} ({resolved}) accepted TCP in {ms}ms")
        }
        Ok(Err(e)) => {
            let kind = classify_error(&e);
            format!("Unreachable — {target}: {kind} ({e})")
        }
        Err(_) => {
            let ms = timeout.as_millis();
            format!("Unreachable — {target}: timed out after {ms}ms")
        }
    }
}

fn classify_error(e: &std::io::Error) -> &'static str {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::ConnectionRefused => "connection refused",
        K::TimedOut => "timed out",
        K::HostUnreachable | K::NetworkUnreachable => "host or network unreachable",
        K::AddrNotAvailable => "address not available",
        K::PermissionDenied => "permission denied",
        _ => "connection failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- parsing -----

    #[test]
    fn parse_simple_host_port() {
        let p = parse_input("example.com:443").unwrap();
        assert_eq!(
            p,
            Parsed {
                host: "example.com".into(),
                port: 443,
                timeout_ms: DEFAULT_TIMEOUT_MS,
            }
        );
    }

    #[test]
    fn parse_localhost_port() {
        let p = parse_input("localhost:8080").unwrap();
        assert_eq!(p.host, "localhost");
        assert_eq!(p.port, 8080);
    }

    #[test]
    fn parse_ipv4_port() {
        let p = parse_input("127.0.0.1:22").unwrap();
        assert_eq!(p.host, "127.0.0.1");
        assert_eq!(p.port, 22);
    }

    #[test]
    fn parse_ipv6_bracketed() {
        let p = parse_input("[::1]:8080").unwrap();
        assert_eq!(p.host, "::1");
        assert_eq!(p.port, 8080);
    }

    #[test]
    fn parse_ipv6_bracketed_without_port_rejected() {
        let err = parse_input("[::1]").unwrap_err();
        assert!(err.contains("missing port"), "got: {err}");
    }

    #[test]
    fn parse_with_timeout() {
        let p = parse_input("example.com:443 timeout=500").unwrap();
        assert_eq!(p.timeout_ms, 500);
    }

    #[test]
    fn parse_timeout_zero_rejected() {
        let err = parse_input("a:1 timeout=0").unwrap_err();
        assert!(err.contains("> 0"), "got: {err}");
    }

    #[test]
    fn parse_timeout_too_large_rejected() {
        let err = parse_input("a:1 timeout=999999").unwrap_err();
        assert!(err.contains("exceeds maximum"), "got: {err}");
    }

    #[test]
    fn parse_timeout_nonnumeric_rejected() {
        let err = parse_input("a:1 timeout=fast").unwrap_err();
        assert!(err.contains("invalid timeout"), "got: {err}");
    }

    #[test]
    fn parse_empty_rejected() {
        let err = parse_input("").unwrap_err();
        assert!(err.contains("empty input"), "got: {err}");
    }

    #[test]
    fn parse_missing_port_rejected() {
        let err = parse_input("example.com").unwrap_err();
        assert!(err.contains("missing port"), "got: {err}");
    }

    #[test]
    fn parse_port_out_of_range_rejected() {
        let err = parse_input("a:70000").unwrap_err();
        assert!(err.contains("out of range"), "got: {err}");
    }

    #[test]
    fn parse_port_zero_rejected() {
        let err = parse_input("a:0").unwrap_err();
        assert!(err.contains("out of range"), "got: {err}");
    }

    #[test]
    fn parse_https_url_infers_port_443() {
        let p = parse_input("https://example.com").unwrap();
        assert_eq!(p.host, "example.com");
        assert_eq!(p.port, 443);
    }

    #[test]
    fn parse_http_url_infers_port_80() {
        let p = parse_input("http://example.com").unwrap();
        assert_eq!(p.host, "example.com");
        assert_eq!(p.port, 80);
    }

    #[test]
    fn parse_https_url_with_explicit_port() {
        let p = parse_input("https://example.com:8443").unwrap();
        assert_eq!(p.host, "example.com");
        assert_eq!(p.port, 8443);
    }

    #[test]
    fn parse_tcp_scheme_stripped() {
        let p = parse_input("tcp://127.0.0.1:22").unwrap();
        assert_eq!(p.host, "127.0.0.1");
        assert_eq!(p.port, 22);
    }

    #[test]
    fn parse_url_path_ignored() {
        let p = parse_input("https://example.com/foo/bar").unwrap();
        assert_eq!(p.host, "example.com");
        assert_eq!(p.port, 443);
    }

    #[test]
    fn parse_null_byte_rejected() {
        let err = parse_input("a\0:1").unwrap_err();
        assert!(err.contains("null byte"), "got: {err}");
    }

    #[test]
    fn parse_extra_token_rejected() {
        let err = parse_input("a:1 foo").unwrap_err();
        assert!(err.contains("unexpected token"), "got: {err}");
    }

    // ----- end-to-end probe -----

    #[tokio::test]
    async fn empty_input_reports_error() {
        let r = tool_check_port("").await;
        assert!(r.starts_with("Error:"), "got: {r}");
    }

    #[tokio::test]
    async fn unresolvable_host_reports_dns_failure() {
        let r = tool_check_port("aictl-nonexistent-xyz-zzz.invalid:80 timeout=500").await;
        assert!(r.starts_with("Unreachable"), "got: {r}");
        assert!(r.contains("DNS") || r.contains("failed"), "got: {r}");
    }

    #[tokio::test]
    async fn closed_local_port_reports_unreachable() {
        // Port 1 is reserved and essentially always closed on the loopback.
        // Use a short timeout so the test stays fast even if the OS waits
        // to send an RST.
        let r = tool_check_port("127.0.0.1:1 timeout=500").await;
        assert!(r.starts_with("Unreachable"), "got: {r}");
    }

    #[tokio::test]
    async fn reachable_local_listener_reports_reachable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept in the background so the probe sees a completed handshake.
        tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        let r = tool_check_port(&format!("127.0.0.1:{} timeout=1000", addr.port())).await;
        assert!(r.starts_with("Reachable"), "got: {r}");
        assert!(r.contains("TCP"), "got: {r}");
    }
}
