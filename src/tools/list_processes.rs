//! List running processes with structured filtering.
//!
//! Safer and more predictable than asking the model to pipe `ps aux` through
//! `grep`: the tool invokes `ps` directly via `tokio::process::Command`
//! (no shell, no metacharacter risk), forces `LC_ALL=C` so decimal formats
//! are deterministic, parses the output in-process, and applies filters
//! from a simple `key=value` input.
//!
//! Input format (empty = top 20 by %CPU):
//!
//! ```text
//! name=<substring>         # case-insensitive match on comm + args
//! user=<username>          # exact match
//! pid=<N>                  # exact pid match
//! min_cpu=<N>              # %CPU >= N
//! min_mem=<N>              # %MEM >= N
//! port=<N>                 # PIDs listening on TCP or UDP port N (via lsof)
//! sort=cpu|mem|pid|name    # default cpu (desc); pid/name ascending
//! limit=<N>                # default 20
//! ```
//!
//! Pairs are newline- or whitespace-separated.

use std::collections::HashSet;
use std::fmt::Write as _;

use super::util::truncate_output;

pub(super) async fn tool_list_processes(input: &str) -> String {
    let filters = match parse_filters(input) {
        Ok(f) => f,
        Err(e) => return format!("Error: {e}"),
    };

    let port_pids = if let Some(port) = filters.port {
        match pids_listening_on_port(port).await {
            Ok(set) => Some(set),
            Err(e) => return format!("Error resolving port {port}: {e}"),
        }
    } else {
        None
    };

    let mut rows = match run_ps().await {
        Ok(r) => r,
        Err(e) => return format!("Error running ps: {e}"),
    };

    rows.retain(|r| matches_filters(r, &filters, port_pids.as_ref()));

    sort_rows(&mut rows, filters.sort);

    let limit = filters.limit.unwrap_or(20);
    if rows.len() > limit {
        rows.truncate(limit);
    }

    if rows.is_empty() {
        return "No processes matched the given filters.".to_string();
    }

    let mut out = format_table(&rows);
    truncate_output(&mut out);
    out
}

// ----- filter parsing -----

#[derive(Debug, Default)]
struct Filters {
    name: Option<String>,
    user: Option<String>,
    pid: Option<u32>,
    min_cpu: Option<f32>,
    min_mem: Option<f32>,
    port: Option<u16>,
    sort: SortKey,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default)]
enum SortKey {
    #[default]
    Cpu,
    Mem,
    Pid,
    Name,
}

fn parse_filters(input: &str) -> Result<Filters, String> {
    let mut f = Filters::default();
    for token in input.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            return Err(format!(
                "expected key=value, got '{token}'. Known keys: name, user, pid, min_cpu, min_mem, port, sort, limit"
            ));
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("empty value for '{key}'"));
        }
        match key.as_str() {
            "name" => f.name = Some(value.to_ascii_lowercase()),
            "user" => f.user = Some(value.to_string()),
            "pid" => {
                f.pid = Some(
                    value
                        .parse::<u32>()
                        .map_err(|e| format!("invalid pid '{value}': {e}"))?,
                );
            }
            "min_cpu" => {
                f.min_cpu = Some(
                    value
                        .parse::<f32>()
                        .map_err(|e| format!("invalid min_cpu '{value}': {e}"))?,
                );
            }
            "min_mem" => {
                f.min_mem = Some(
                    value
                        .parse::<f32>()
                        .map_err(|e| format!("invalid min_mem '{value}': {e}"))?,
                );
            }
            "port" => {
                f.port = Some(
                    value
                        .parse::<u16>()
                        .map_err(|e| format!("invalid port '{value}': {e}"))?,
                );
            }
            "sort" => {
                f.sort = match value.to_ascii_lowercase().as_str() {
                    "cpu" => SortKey::Cpu,
                    "mem" => SortKey::Mem,
                    "pid" => SortKey::Pid,
                    "name" => SortKey::Name,
                    other => {
                        return Err(format!(
                            "invalid sort '{other}'. Expected cpu, mem, pid, or name"
                        ));
                    }
                };
            }
            "limit" => {
                f.limit = Some(
                    value
                        .parse::<usize>()
                        .map_err(|e| format!("invalid limit '{value}': {e}"))?,
                );
            }
            _ => {
                return Err(format!(
                    "unknown key '{key}'. Known keys: name, user, pid, min_cpu, min_mem, port, sort, limit"
                ));
            }
        }
    }
    Ok(f)
}

// ----- process row -----

#[derive(Debug)]
struct ProcRow {
    pid: u32,
    user: String,
    cpu: f32,
    mem: f32,
    rss_kb: u64,
    comm: String,
    args: String,
}

fn matches_filters(r: &ProcRow, f: &Filters, port_pids: Option<&HashSet<u32>>) -> bool {
    if let Some(pid) = f.pid
        && r.pid != pid
    {
        return false;
    }
    if let Some(user) = &f.user
        && r.user != *user
    {
        return false;
    }
    if let Some(name) = &f.name {
        let hay_comm = r.comm.to_ascii_lowercase();
        let hay_args = r.args.to_ascii_lowercase();
        if !hay_comm.contains(name) && !hay_args.contains(name) {
            return false;
        }
    }
    if let Some(c) = f.min_cpu
        && r.cpu < c
    {
        return false;
    }
    if let Some(m) = f.min_mem
        && r.mem < m
    {
        return false;
    }
    if let Some(pids) = port_pids
        && !pids.contains(&r.pid)
    {
        return false;
    }
    true
}

fn sort_rows(rows: &mut [ProcRow], key: SortKey) {
    match key {
        SortKey::Cpu => rows.sort_by(|a, b| {
            b.cpu
                .partial_cmp(&a.cpu)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SortKey::Mem => rows.sort_by(|a, b| {
            b.mem
                .partial_cmp(&a.mem)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SortKey::Pid => rows.sort_by_key(|r| r.pid),
        SortKey::Name => rows.sort_by(|a, b| a.comm.cmp(&b.comm)),
    }
}

// ----- ps invocation -----

async fn run_ps() -> Result<Vec<ProcRow>, String> {
    let mut cmd = tokio::process::Command::new("ps");
    cmd.args(["-eo", "pid,user,pcpu,pmem,rss,comm,args"]);

    cmd.env_clear();
    for (k, v) in crate::security::scrubbed_env() {
        cmd.env(k, v);
    }
    // Force deterministic decimal separator ('.') and column widths.
    cmd.env("LC_ALL", "C");
    cmd.env("LANG", "C");

    cmd.current_dir(&crate::security::policy().paths.working_dir);
    cmd.kill_on_drop(true);

    let future = cmd.output();
    let output = if let Some(timeout) = crate::security::shell_timeout() {
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| format!("ps timed out after {}s", timeout.as_secs()))?
            .map_err(|e| format!("spawning ps: {e}"))?
    } else {
        future.await.map_err(|e| format!("spawning ps: {e}"))?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ps exited with status {} — {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }

    parse_ps_output(&String::from_utf8_lossy(&output.stdout))
}

fn parse_ps_output(s: &str) -> Result<Vec<ProcRow>, String> {
    let mut lines = s.lines();
    let header = lines
        .next()
        .ok_or_else(|| "ps produced no output".to_string())?;
    if !header.contains("PID") || (!header.contains("ARGS") && !header.contains("COMMAND")) {
        return Err(format!("unrecognized ps header: {header}"));
    }

    let mut rows = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        // Columns: pid, user, pcpu, pmem, rss, comm, args. `args` is the
        // only field that may contain whitespace, so take the first 6
        // whitespace-separated tokens and treat the remainder as args.
        let parts = split_first_n_plus_rest(line, 6);
        if parts.len() < 6 {
            continue;
        }
        let Ok(pid) = parts[0].parse::<u32>() else {
            continue;
        };
        let user = parts[1].to_string();
        let cpu = parts[2].replace(',', ".").parse::<f32>().unwrap_or(0.0);
        let mem = parts[3].replace(',', ".").parse::<f32>().unwrap_or(0.0);
        let rss_kb = parts[4].parse::<u64>().unwrap_or(0);
        let comm = parts[5].to_string();
        let args = parts.get(6).map(|s| (*s).to_string()).unwrap_or_default();
        rows.push(ProcRow {
            pid,
            user,
            cpu,
            mem,
            rss_kb,
            comm,
            args,
        });
    }
    Ok(rows)
}

/// Split `line` into up to `n` leading whitespace-separated tokens plus a
/// trailing remainder (leading whitespace stripped). Runs of whitespace are
/// collapsed like `split_whitespace`, so column-aligned ps output parses
/// cleanly without empty tokens between columns.
fn split_first_n_plus_rest(line: &str, n: usize) -> Vec<&str> {
    let mut parts = Vec::with_capacity(n + 1);
    let mut rest = line;
    for _ in 0..n {
        rest = rest.trim_start();
        if rest.is_empty() {
            return parts;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        parts.push(&rest[..end]);
        rest = &rest[end..];
    }
    rest = rest.trim_start();
    if !rest.is_empty() {
        parts.push(rest);
    }
    parts
}

// ----- port resolution via lsof -----

async fn pids_listening_on_port(port: u16) -> Result<HashSet<u32>, String> {
    let mut cmd = tokio::process::Command::new("lsof");
    cmd.args(["-nP", "-iTCP", "-iUDP", "-a", &format!("-i:{port}")]);

    cmd.env_clear();
    for (k, v) in crate::security::scrubbed_env() {
        cmd.env(k, v);
    }
    cmd.env("LC_ALL", "C");

    cmd.current_dir(&crate::security::policy().paths.working_dir);
    cmd.kill_on_drop(true);

    let future = cmd.output();
    let output = if let Some(timeout) = crate::security::shell_timeout() {
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| format!("lsof timed out after {}s", timeout.as_secs()))?
            .map_err(|e| format!("spawning lsof: {e} (is lsof installed?)"))?
    } else {
        future
            .await
            .map_err(|e| format!("spawning lsof: {e} (is lsof installed?)"))?
    };

    // lsof exits 1 when no matches — treat as empty set, not error.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut pids = HashSet::new();
    for (i, line) in stdout.lines().enumerate() {
        if i == 0 {
            continue; // header
        }
        let mut fields = line.split_whitespace();
        let _ = fields.next(); // COMMAND
        if let Some(pid_s) = fields.next()
            && let Ok(pid) = pid_s.parse::<u32>()
        {
            pids.insert(pid);
        }
    }
    Ok(pids)
}

// ----- table formatting -----

fn format_table(rows: &[ProcRow]) -> String {
    let headers = ["PID", "USER", "%CPU", "%MEM", "RSS", "COMMAND"];
    let body: Vec<[String; 6]> = rows
        .iter()
        .map(|r| {
            [
                r.pid.to_string(),
                r.user.clone(),
                format!("{:.1}", r.cpu),
                format!("{:.1}", r.mem),
                format_rss(r.rss_kb),
                pick_command(r),
            ]
        })
        .collect();

    let mut widths = headers.map(str::len);
    for row in &body {
        for (i, cell) in row.iter().enumerate() {
            let w = cell.chars().count();
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }

    let mut out = String::new();
    out.push('|');
    for (i, h) in headers.iter().enumerate() {
        let pad = widths[i].saturating_sub(h.len());
        let _ = write!(out, " {h}{} |", " ".repeat(pad));
    }
    out.push('\n');
    out.push('|');
    for w in widths {
        out.push_str(&"-".repeat(w + 2));
        out.push('|');
    }
    out.push('\n');
    for row in &body {
        out.push('|');
        for (i, cell) in row.iter().enumerate() {
            let pad = widths[i].saturating_sub(cell.chars().count());
            let _ = write!(out, " {cell}{} |", " ".repeat(pad));
        }
        out.push('\n');
    }
    let n = rows.len();
    let _ = writeln!(
        out,
        "({n} {})",
        if n == 1 { "process" } else { "processes" }
    );
    out
}

fn pick_command(r: &ProcRow) -> String {
    if r.args.is_empty() {
        r.comm.clone()
    } else {
        r.args.clone()
    }
}

fn format_rss(kb: u64) -> String {
    if kb >= 1024 * 1024 {
        #[allow(clippy::cast_precision_loss)]
        let gb = kb as f64 / (1024.0 * 1024.0);
        format!("{gb:.1}G")
    } else if kb >= 1024 {
        #[allow(clippy::cast_precision_loss)]
        let mb = kb as f64 / 1024.0;
        format!("{mb:.1}M")
    } else {
        format!("{kb}K")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_filters_empty() {
        let f = parse_filters("").unwrap();
        assert!(f.name.is_none());
        assert!(f.limit.is_none());
        assert!(matches!(f.sort, SortKey::Cpu));
    }

    #[test]
    fn parse_filters_all_keys() {
        let f = parse_filters(
            "name=chrome user=pw pid=42 min_cpu=1.5 min_mem=2 port=8080 sort=mem limit=5",
        )
        .unwrap();
        assert_eq!(f.name.unwrap(), "chrome");
        assert_eq!(f.user.unwrap(), "pw");
        assert_eq!(f.pid.unwrap(), 42);
        assert!((f.min_cpu.unwrap() - 1.5).abs() < f32::EPSILON);
        assert!((f.min_mem.unwrap() - 2.0).abs() < f32::EPSILON);
        assert_eq!(f.port.unwrap(), 8080);
        assert!(matches!(f.sort, SortKey::Mem));
        assert_eq!(f.limit.unwrap(), 5);
    }

    #[test]
    fn parse_filters_newline_separated() {
        let f = parse_filters("name=node\nlimit=3").unwrap();
        assert_eq!(f.name.unwrap(), "node");
        assert_eq!(f.limit.unwrap(), 3);
    }

    #[test]
    fn parse_filters_lowercases_name() {
        let f = parse_filters("name=ChRoMe").unwrap();
        assert_eq!(f.name.unwrap(), "chrome");
    }

    #[test]
    fn parse_filters_rejects_unknown_key() {
        let err = parse_filters("unknown=1").unwrap_err();
        assert!(err.contains("unknown key"), "got: {err}");
    }

    #[test]
    fn parse_filters_rejects_missing_equals() {
        let err = parse_filters("foo").unwrap_err();
        assert!(err.contains("key=value"), "got: {err}");
    }

    #[test]
    fn parse_filters_rejects_bad_pid() {
        let err = parse_filters("pid=abc").unwrap_err();
        assert!(err.contains("invalid pid"), "got: {err}");
    }

    #[test]
    fn parse_filters_rejects_bad_sort() {
        let err = parse_filters("sort=size").unwrap_err();
        assert!(err.contains("invalid sort"), "got: {err}");
    }

    #[test]
    fn parse_filters_rejects_empty_value() {
        let err = parse_filters("name=").unwrap_err();
        assert!(err.contains("empty value"), "got: {err}");
    }

    #[test]
    fn parse_ps_output_basic() {
        let out = "  PID USER               %CPU %MEM    RSS COMM             ARGS\n\
                      1 root                0.1  0.1  24736 /sbin/launchd    /sbin/launchd\n\
                    337 root                0.3  0.2  65680 /usr/libexec/log /usr/libexec/logd --config\n";
        let rows = parse_ps_output(out).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pid, 1);
        assert_eq!(rows[0].user, "root");
        assert!((rows[0].cpu - 0.1).abs() < 0.01);
        assert!((rows[0].mem - 0.1).abs() < 0.01);
        assert_eq!(rows[0].rss_kb, 24736);
        assert!(rows[0].comm.contains("launchd"));
        assert!(rows[0].args.contains("/sbin/launchd"));
        assert_eq!(rows[1].pid, 337);
        assert!(rows[1].args.contains("logd --config"));
    }

    #[test]
    fn parse_ps_output_comma_decimal() {
        // Some locales render %CPU/%MEM with commas; we force LC_ALL=C but
        // if that ever fails we still want a sensible fallback.
        let out = "  PID USER               %CPU %MEM    RSS COMM             ARGS\n\
                      1 root                0,5  1,2   1024 foo              foo\n";
        let rows = parse_ps_output(out).unwrap();
        assert_eq!(rows.len(), 1);
        assert!((rows[0].cpu - 0.5).abs() < 0.01);
        assert!((rows[0].mem - 1.2).abs() < 0.01);
    }

    #[test]
    fn parse_ps_output_empty_rejected() {
        let err = parse_ps_output("").unwrap_err();
        assert!(err.contains("no output"), "got: {err}");
    }

    #[test]
    fn parse_ps_output_bad_header() {
        let err = parse_ps_output("no columns here\n").unwrap_err();
        assert!(err.contains("unrecognized ps header"), "got: {err}");
    }

    fn row(pid: u32, user: &str, cpu: f32, mem: f32, comm: &str, args: &str) -> ProcRow {
        ProcRow {
            pid,
            user: user.to_string(),
            cpu,
            mem,
            rss_kb: 0,
            comm: comm.to_string(),
            args: args.to_string(),
        }
    }

    #[test]
    fn matches_filters_name_substring_case_insensitive() {
        let r = row(1, "pw", 0.0, 0.0, "/usr/bin/Chrome", "Chrome --foo");
        let f = parse_filters("name=chrome").unwrap();
        assert!(matches_filters(&r, &f, None));
    }

    #[test]
    fn matches_filters_name_matches_in_args() {
        let r = row(1, "pw", 0.0, 0.0, "node", "node /path/to/server.js");
        let f = parse_filters("name=server.js").unwrap();
        assert!(matches_filters(&r, &f, None));
    }

    #[test]
    fn matches_filters_user_exact() {
        let r = row(1, "pw", 0.0, 0.0, "foo", "foo");
        let match_f = parse_filters("user=pw").unwrap();
        let miss_f = parse_filters("user=root").unwrap();
        assert!(matches_filters(&r, &match_f, None));
        assert!(!matches_filters(&r, &miss_f, None));
    }

    #[test]
    fn matches_filters_min_cpu() {
        let r = row(1, "pw", 5.0, 0.0, "foo", "foo");
        let f_hit = parse_filters("min_cpu=4").unwrap();
        let f_miss = parse_filters("min_cpu=10").unwrap();
        assert!(matches_filters(&r, &f_hit, None));
        assert!(!matches_filters(&r, &f_miss, None));
    }

    #[test]
    fn matches_filters_port_set() {
        let r1 = row(10, "pw", 0.0, 0.0, "a", "a");
        let r2 = row(20, "pw", 0.0, 0.0, "b", "b");
        let mut pids = HashSet::new();
        pids.insert(10);
        let f = parse_filters("port=8080").unwrap();
        assert!(matches_filters(&r1, &f, Some(&pids)));
        assert!(!matches_filters(&r2, &f, Some(&pids)));
    }

    #[test]
    fn sort_rows_by_cpu_descending() {
        let mut rs = vec![
            row(1, "pw", 1.0, 0.0, "a", "a"),
            row(2, "pw", 9.0, 0.0, "b", "b"),
            row(3, "pw", 5.0, 0.0, "c", "c"),
        ];
        sort_rows(&mut rs, SortKey::Cpu);
        assert_eq!(rs[0].pid, 2);
        assert_eq!(rs[1].pid, 3);
        assert_eq!(rs[2].pid, 1);
    }

    #[test]
    fn sort_rows_by_pid_ascending() {
        let mut rs = vec![
            row(30, "pw", 0.0, 0.0, "a", "a"),
            row(10, "pw", 0.0, 0.0, "b", "b"),
            row(20, "pw", 0.0, 0.0, "c", "c"),
        ];
        sort_rows(&mut rs, SortKey::Pid);
        assert_eq!(rs[0].pid, 10);
        assert_eq!(rs[1].pid, 20);
        assert_eq!(rs[2].pid, 30);
    }

    #[test]
    fn format_rss_units() {
        assert_eq!(format_rss(512), "512K");
        assert_eq!(format_rss(2048), "2.0M");
        assert_eq!(format_rss(2 * 1024 * 1024), "2.0G");
    }

    #[test]
    fn format_table_renders_header_and_row_count() {
        let rows = vec![row(1, "pw", 1.0, 2.0, "foo", "foo --bar")];
        let out = format_table(&rows);
        assert!(out.contains("PID"));
        assert!(out.contains("COMMAND"));
        assert!(out.contains("foo --bar"));
        assert!(out.contains("(1 process)"));
    }

    #[tokio::test]
    async fn tool_rejects_bad_filter() {
        let r = tool_list_processes("bogus=1").await;
        assert!(r.starts_with("Error:"), "got: {r}");
        assert!(r.contains("unknown key"), "got: {r}");
    }

    #[tokio::test]
    async fn tool_lists_current_process() {
        // pid=1 should exist on every Unix-like system; this also exercises
        // the real `ps` subprocess path end-to-end.
        let r = tool_list_processes("pid=1").await;
        assert!(r.contains("PID"), "got: {r}");
        assert!(r.contains("(1 process)"), "got: {r}");
    }
}
