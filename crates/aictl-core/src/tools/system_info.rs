//! Return structured OS, CPU, memory, and disk information.
//!
//! Gives the model a single, predictable call for capability/environment
//! probing instead of spraying the session with `uname -a`, `df -h`,
//! `sysctl …`, `cat /proc/cpuinfo`, etc. Cross-platform for macOS (via
//! `sysctl` + `vm_stat`) and Linux (via `/proc/*` reads); unknown
//! platforms fall back to whatever the Rust standard library can tell us
//! (OS name, arch, logical core count).
//!
//! Input format (empty = all sections):
//!
//! ```text
//! section=<os|cpu|memory|disk|all>   # default: all
//! path=<directory>                    # disk section only; default: security working dir
//! ```

use std::collections::HashSet;
use std::fmt::Write as _;

use super::util::truncate_output;

pub(super) async fn tool_system_info(input: &str) -> String {
    let opts = match parse_options(input) {
        Ok(o) => o,
        Err(e) => return format!("Error: {e}"),
    };

    let workdir = crate::security::policy()
        .paths
        .working_dir
        .to_string_lossy()
        .into_owned();
    let disk_path = opts.path.clone().unwrap_or(workdir);

    let mut out = String::new();

    if opts.sections.contains(&Section::Os) {
        out.push_str(&render_os().await);
        out.push('\n');
    }
    if opts.sections.contains(&Section::Cpu) {
        out.push_str(&render_cpu().await);
        out.push('\n');
    }
    if opts.sections.contains(&Section::Memory) {
        out.push_str(&render_memory().await);
        out.push('\n');
    }
    if opts.sections.contains(&Section::Disk) {
        out.push_str(&render_disk(&disk_path).await);
        out.push('\n');
    }

    let mut s = out.trim_end().to_string();
    truncate_output(&mut s);
    s
}

// ----- options -----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Os,
    Cpu,
    Memory,
    Disk,
}

const ALL_SECTIONS: &[Section] = &[Section::Os, Section::Cpu, Section::Memory, Section::Disk];

#[derive(Debug)]
struct Options {
    sections: Vec<Section>,
    path: Option<String>,
}

fn parse_options(input: &str) -> Result<Options, String> {
    let mut sections: Option<Vec<Section>> = None;
    let mut path: Option<String> = None;

    for token in input.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            return Err(format!(
                "expected key=value, got '{token}'. Known keys: section, path"
            ));
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("empty value for '{key}'"));
        }
        match key.as_str() {
            "section" => {
                sections = Some(match value.to_ascii_lowercase().as_str() {
                    "all" => ALL_SECTIONS.to_vec(),
                    "os" => vec![Section::Os],
                    "cpu" => vec![Section::Cpu],
                    "memory" | "mem" => vec![Section::Memory],
                    "disk" => vec![Section::Disk],
                    other => {
                        return Err(format!(
                            "invalid section '{other}'. Expected one of: os, cpu, memory, disk, all"
                        ));
                    }
                });
            }
            "path" => path = Some(value.to_string()),
            _ => return Err(format!("unknown key '{key}'. Known keys: section, path")),
        }
    }

    Ok(Options {
        sections: sections.unwrap_or_else(|| ALL_SECTIONS.to_vec()),
        path,
    })
}

// ----- subprocess helper -----

async fn run_quiet(bin: &str, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args);
    cmd.env_clear();
    for (k, v) in crate::security::scrubbed_env() {
        cmd.env(k, v);
    }
    cmd.env("LC_ALL", "C");
    cmd.env("LANG", "C");
    cmd.current_dir(&crate::security::policy().paths.working_dir);
    cmd.kill_on_drop(true);

    let future = cmd.output();
    let output = if let Some(timeout) = crate::security::shell_timeout() {
        tokio::time::timeout(timeout, future).await.ok()?.ok()?
    } else {
        future.await.ok()?
    };

    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ----- OS -----

async fn render_os() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let kernel = run_quiet("uname", &["-sr"]).await;
    let hostname = run_quiet("uname", &["-n"]).await;

    let pretty_os = match os {
        "linux" => linux_os_pretty().await,
        "macos" => macos_product_version().await,
        _ => None,
    };

    let mut s = String::from("## OS\n");
    match pretty_os {
        Some(p) => {
            let _ = writeln!(s, "- Name: {p} ({os})");
        }
        None => {
            let _ = writeln!(s, "- Name: {os}");
        }
    }
    let _ = writeln!(s, "- Arch: {arch}");
    if let Some(k) = kernel.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        let _ = writeln!(s, "- Kernel: {k}");
    }
    if let Some(h) = hostname.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        let _ = writeln!(s, "- Hostname: {h}");
    }
    s
}

async fn linux_os_pretty() -> Option<String> {
    let contents = tokio::fs::read_to_string("/etc/os-release").await.ok()?;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
            return Some(rest.trim_matches('"').to_string());
        }
    }
    None
}

async fn macos_product_version() -> Option<String> {
    let out = run_quiet("sw_vers", &["-productVersion"]).await?;
    let version = out.trim();
    if version.is_empty() {
        return None;
    }
    Some(format!("macOS {version}"))
}

// ----- CPU -----

async fn render_cpu() -> String {
    let mut s = String::from("## CPU\n");
    let model = match std::env::consts::OS {
        "macos" => run_quiet("sysctl", &["-n", "machdep.cpu.brand_string"]).await,
        "linux" => linux_cpu_model().await,
        _ => None,
    };
    if let Some(m) = model.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        let _ = writeln!(s, "- Model: {m}");
    }
    if let Ok(n) = std::thread::available_parallelism().map(std::num::NonZero::get) {
        let _ = writeln!(s, "- Logical cores: {n}");
    }
    let physical = match std::env::consts::OS {
        "macos" => run_quiet("sysctl", &["-n", "hw.physicalcpu"])
            .await
            .and_then(|s| s.trim().parse::<usize>().ok()),
        "linux" => linux_physical_cores().await,
        _ => None,
    };
    if let Some(p) = physical {
        let _ = writeln!(s, "- Physical cores: {p}");
    }
    s
}

async fn linux_cpu_model() -> Option<String> {
    let contents = tokio::fs::read_to_string("/proc/cpuinfo").await.ok()?;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("model name") {
            return Some(
                rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace())
                    .to_string(),
            );
        }
        // ARM boards often report `Hardware` or `Model` instead.
        if let Some(rest) = line.strip_prefix("Hardware") {
            return Some(
                rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace())
                    .to_string(),
            );
        }
    }
    None
}

async fn linux_physical_cores() -> Option<usize> {
    let contents = tokio::fs::read_to_string("/proc/cpuinfo").await.ok()?;
    let mut set: HashSet<(String, String)> = HashSet::new();
    let mut phys: Option<String> = None;
    let mut core: Option<String> = None;
    for line in contents.lines() {
        if line.trim().is_empty() {
            if let (Some(p), Some(c)) = (phys.take(), core.take()) {
                set.insert((p, c));
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("physical id") {
            phys = Some(
                rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace())
                    .to_string(),
            );
        } else if let Some(rest) = line.strip_prefix("core id") {
            core = Some(
                rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace())
                    .to_string(),
            );
        }
    }
    if let (Some(p), Some(c)) = (phys, core) {
        set.insert((p, c));
    }
    if set.is_empty() {
        None
    } else {
        Some(set.len())
    }
}

// ----- memory -----

async fn render_memory() -> String {
    let mut s = String::from("## Memory\n");
    match std::env::consts::OS {
        "linux" => {
            if let Some(info) = linux_meminfo().await {
                let used = info.total_kb.saturating_sub(info.available_kb);
                let _ = writeln!(s, "- Total: {}", format_kb(info.total_kb));
                let _ = writeln!(
                    s,
                    "- Used: {} ({:.1}%)",
                    format_kb(used),
                    pct(used, info.total_kb)
                );
                let _ = writeln!(s, "- Available: {}", format_kb(info.available_kb));
            } else {
                s.push_str("- (unavailable — could not read /proc/meminfo)\n");
            }
        }
        "macos" => {
            let total = run_quiet("sysctl", &["-n", "hw.memsize"])
                .await
                .and_then(|v| v.trim().parse::<u64>().ok());
            let Some(total) = total else {
                s.push_str("- (unavailable — sysctl hw.memsize failed)\n");
                return s;
            };
            let _ = writeln!(s, "- Total: {}", format_bytes(total));
            if let Some(free_bytes) = macos_free_memory().await {
                let used = total.saturating_sub(free_bytes);
                let _ = writeln!(
                    s,
                    "- Used: {} ({:.1}%)",
                    format_bytes(used),
                    pct(used, total)
                );
                let _ = writeln!(s, "- Free: {}", format_bytes(free_bytes));
            }
        }
        _ => s.push_str("- (unsupported platform)\n"),
    }
    s
}

struct MemInfo {
    total_kb: u64,
    available_kb: u64,
}

async fn linux_meminfo() -> Option<MemInfo> {
    let contents = tokio::fs::read_to_string("/proc/meminfo").await.ok()?;
    let mut total = None;
    let mut avail = None;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = parse_kb_field(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail = parse_kb_field(rest);
        }
    }
    Some(MemInfo {
        total_kb: total?,
        available_kb: avail?,
    })
}

fn parse_kb_field(rest: &str) -> Option<u64> {
    rest.split_whitespace().next()?.parse::<u64>().ok()
}

async fn macos_free_memory() -> Option<u64> {
    let out = run_quiet("vm_stat", &[]).await?;
    let mut page_size: u64 = 4096;
    let mut free_pages: u64 = 0;
    let mut inactive_pages: u64 = 0;
    let mut speculative_pages: u64 = 0;
    for line in out.lines() {
        if line.starts_with("Mach Virtual Memory Statistics") {
            if let Some(idx) = line.find("page size of ") {
                let rest = &line[idx + "page size of ".len()..];
                if let Some(num_s) = rest.split_whitespace().next()
                    && let Ok(n) = num_s.parse::<u64>()
                {
                    page_size = n;
                }
            }
            continue;
        }
        if let Some(val) = parse_vm_stat_line(line, "Pages free:") {
            free_pages = val;
        } else if let Some(val) = parse_vm_stat_line(line, "Pages inactive:") {
            inactive_pages = val;
        } else if let Some(val) = parse_vm_stat_line(line, "Pages speculative:") {
            speculative_pages = val;
        }
    }
    Some((free_pages + inactive_pages + speculative_pages) * page_size)
}

fn parse_vm_stat_line(line: &str, prefix: &str) -> Option<u64> {
    let rest = line.trim().strip_prefix(prefix)?;
    let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok()
}

// ----- disk -----

async fn render_disk(path: &str) -> String {
    let mut s = String::from("## Disk\n");
    let Some(out) = run_quiet("df", &["-kP", path]).await else {
        let _ = writeln!(s, "- Path: {path}");
        s.push_str("- (unavailable — `df -kP` failed)\n");
        return s;
    };
    let mut lines = out.lines();
    let _ = lines.next(); // header
    let Some(line) = lines.next() else {
        s.push_str("- (no data from df)\n");
        return s;
    };

    // `df -kP` columns: Filesystem 1024-blocks Used Available Capacity Mounted-on
    // Mount point is the last column and may contain spaces; take leading 5
    // whitespace-separated tokens plus rest.
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 6 {
        s.push_str("- (unparseable df output)\n");
        return s;
    }
    let filesystem = parts[0];
    let total_kb: u64 = parts[1].parse().unwrap_or(0);
    let used_kb: u64 = parts[2].parse().unwrap_or(0);
    let avail_kb: u64 = parts[3].parse().unwrap_or(0);
    let capacity = parts[4];
    let mount = parts[5..].join(" ");

    let _ = writeln!(s, "- Path: {path}");
    let _ = writeln!(s, "- Mount: {mount}");
    let _ = writeln!(s, "- Filesystem: {filesystem}");
    let _ = writeln!(s, "- Total: {}", format_kb(total_kb));
    let _ = writeln!(s, "- Used: {} ({capacity})", format_kb(used_kb));
    let _ = writeln!(s, "- Available: {}", format_kb(avail_kb));
    s
}

// ----- formatting helpers -----

fn format_kb(kb: u64) -> String {
    format_bytes(kb.saturating_mul(1024))
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    if b >= TB {
        format!("{:.2} TB", b / TB)
    } else if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        let r = part as f64 / whole as f64 * 100.0;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_options_empty_defaults_to_all() {
        let o = parse_options("").unwrap();
        assert_eq!(o.sections, ALL_SECTIONS.to_vec());
        assert!(o.path.is_none());
    }

    #[test]
    fn parse_options_section_cpu() {
        let o = parse_options("section=cpu").unwrap();
        assert_eq!(o.sections, vec![Section::Cpu]);
    }

    #[test]
    fn parse_options_section_memory_alias() {
        let o = parse_options("section=mem").unwrap();
        assert_eq!(o.sections, vec![Section::Memory]);
    }

    #[test]
    fn parse_options_section_all() {
        let o = parse_options("section=all").unwrap();
        assert_eq!(o.sections, ALL_SECTIONS.to_vec());
    }

    #[test]
    fn parse_options_path() {
        let o = parse_options("section=disk path=/tmp").unwrap();
        assert_eq!(o.sections, vec![Section::Disk]);
        assert_eq!(o.path.as_deref(), Some("/tmp"));
    }

    #[test]
    fn parse_options_rejects_unknown_key() {
        let err = parse_options("weird=1").unwrap_err();
        assert!(err.contains("unknown key"), "got: {err}");
    }

    #[test]
    fn parse_options_rejects_missing_equals() {
        let err = parse_options("section").unwrap_err();
        assert!(err.contains("key=value"), "got: {err}");
    }

    #[test]
    fn parse_options_rejects_bad_section() {
        let err = parse_options("section=weird").unwrap_err();
        assert!(err.contains("invalid section"), "got: {err}");
    }

    #[test]
    fn parse_options_rejects_empty_value() {
        let err = parse_options("section=").unwrap_err();
        assert!(err.contains("empty value"), "got: {err}");
    }

    #[test]
    fn format_bytes_ranges() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2 * 1024), "2.0 KB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[test]
    fn format_kb_converts() {
        assert_eq!(format_kb(1024), "1.0 MB");
    }

    #[test]
    fn pct_safe_for_zero_denominator() {
        assert!((pct(5, 0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn pct_half() {
        assert!((pct(50, 100) - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_kb_field_basic() {
        assert_eq!(parse_kb_field("   16384 kB"), Some(16384));
    }

    #[test]
    fn parse_kb_field_rejects_non_numeric() {
        assert_eq!(parse_kb_field("   foo kB"), None);
    }

    #[test]
    fn parse_vm_stat_line_extracts_number() {
        assert_eq!(
            parse_vm_stat_line("Pages free:         12345.", "Pages free:"),
            Some(12345)
        );
    }

    #[test]
    fn parse_vm_stat_line_wrong_prefix() {
        assert_eq!(
            parse_vm_stat_line("Pages active:       99.", "Pages free:"),
            None
        );
    }

    #[tokio::test]
    async fn tool_system_info_runs_default() {
        let r = tool_system_info("").await;
        assert!(r.contains("## OS"), "got: {r}");
        assert!(r.contains("Arch:"), "got: {r}");
        assert!(r.contains("## CPU"), "got: {r}");
        assert!(r.contains("## Memory"), "got: {r}");
        assert!(r.contains("## Disk"), "got: {r}");
    }

    #[tokio::test]
    async fn tool_system_info_single_section() {
        let r = tool_system_info("section=os").await;
        assert!(r.contains("## OS"), "got: {r}");
        assert!(!r.contains("## CPU"), "got: {r}");
        assert!(!r.contains("## Memory"), "got: {r}");
        assert!(!r.contains("## Disk"), "got: {r}");
    }

    #[tokio::test]
    async fn tool_system_info_rejects_bad_input() {
        let r = tool_system_info("bogus=1").await;
        assert!(r.starts_with("Error:"), "got: {r}");
    }
}
