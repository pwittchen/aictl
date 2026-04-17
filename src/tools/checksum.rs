//! Compute cryptographic checksums (SHA-256, MD5) of a file.
//!
//! Useful for verifying downloads, matching published hashes, and spot-checking
//! file integrity without shelling out to `shasum` / `sha256sum` / `md5sum`,
//! whose binary names and flags vary by platform.
//!
//! Input shapes accepted:
//!
//! ```text
//! <path>
//! ```
//!
//! computes *both* SHA-256 and MD5 of the file. To request a single algorithm,
//! prefix the line with the algorithm name:
//!
//! ```text
//! sha256 <path>
//! md5 <path>
//! ```
//!
//! Files are streamed through the hashers in 64 KB chunks so arbitrarily large
//! inputs can be hashed without loading the whole file into memory. Hashing
//! itself runs on `tokio::spawn_blocking` so a slow disk or huge file doesn't
//! stall the async runtime.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use md5::Md5;
use sha2::{Digest, Sha256};

const CHUNK_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy)]
enum Algo {
    Sha256,
    Md5,
    Both,
}

pub(super) async fn tool_checksum(input: &str) -> String {
    let input = input.trim().to_string();
    tokio::task::spawn_blocking(move || run(&input))
        .await
        .unwrap_or_else(|e| format!("Error running checksum: {e}"))
}

fn run(input: &str) -> String {
    let (algo, path) = match parse_input(input) {
        Ok(pair) => pair,
        Err(e) => return e,
    };

    if path.is_empty() {
        return "Invalid input: no file path supplied".to_string();
    }

    let p = Path::new(path);
    if !p.exists() {
        return format!("Error: file does not exist: {path}");
    }
    if !p.is_file() {
        return format!("Error: not a regular file: {path}");
    }

    match hash_file(p, algo) {
        Ok((sha, md5)) => format_result(algo, sha.as_deref(), md5.as_deref()),
        Err(e) => format!("Error reading file: {e}"),
    }
}

fn parse_input(input: &str) -> Result<(Algo, &str), String> {
    let input = input.trim();
    // Only inspect the first line; any trailing lines are ignored so a model
    // that accidentally adds notes below the path still works.
    let first_line = input.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return Err(
            "Invalid input: expected a file path (optionally prefixed with `sha256` or `md5`)"
                .to_string(),
        );
    }

    // Check for `<algo> <path>` form. The algorithm token must be the first
    // whitespace-separated word — if it isn't a recognized algo we treat the
    // whole line as a path, which keeps files named `sha256_notes.txt` etc.
    // working without an explicit prefix.
    if let Some((head, tail)) = first_line.split_once(char::is_whitespace) {
        let head_lower = head.to_ascii_lowercase();
        let tail = tail.trim();
        match head_lower.as_str() {
            "sha256" | "sha-256" => return Ok((Algo::Sha256, tail)),
            "md5" => return Ok((Algo::Md5, tail)),
            "both" | "all" => return Ok((Algo::Both, tail)),
            _ => {}
        }
    }

    Ok((Algo::Both, first_line))
}

fn hash_file(path: &Path, algo: Algo) -> std::io::Result<(Option<String>, Option<String>)> {
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; CHUNK_SIZE];

    let mut sha = matches!(algo, Algo::Sha256 | Algo::Both).then(Sha256::new);
    let mut md5 = matches!(algo, Algo::Md5 | Algo::Both).then(Md5::new);

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if let Some(h) = sha.as_mut() {
            h.update(&buf[..n]);
        }
        if let Some(h) = md5.as_mut() {
            h.update(&buf[..n]);
        }
    }

    let sha_hex = sha.map(|h| hex_lower(&h.finalize()));
    let md5_hex = md5.map(|h| hex_lower(&h.finalize()));
    Ok((sha_hex, md5_hex))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(nibble(b >> 4));
        s.push(nibble(b & 0x0f));
    }
    s
}

fn nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'a' + n - 10) as char,
        _ => unreachable!(),
    }
}

fn format_result(algo: Algo, sha: Option<&str>, md5: Option<&str>) -> String {
    match algo {
        Algo::Sha256 => format!("SHA-256: {}", sha.unwrap_or("")),
        Algo::Md5 => format!("MD5: {}", md5.unwrap_or("")),
        Algo::Both => format!(
            "SHA-256: {}\nMD5:     {}",
            sha.unwrap_or(""),
            md5.unwrap_or("")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aictl_checksum_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // Known test vectors: "abc" in UTF-8.
    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const ABC_MD5: &str = "900150983cd24fb0d6963f7d28e17f72";
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const EMPTY_MD5: &str = "d41d8cd98f00b204e9800998ecf8427e";

    #[tokio::test]
    async fn computes_both_by_default() {
        let dir = tmp_dir("default");
        let path = dir.join("f.txt");
        std::fs::write(&path, b"abc").unwrap();
        let out = tool_checksum(&path.display().to_string()).await;
        assert!(
            out.contains(&format!("SHA-256: {ABC_SHA256}")),
            "got: {out}"
        );
        assert!(out.contains(&format!("MD5:     {ABC_MD5}")), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn computes_sha256_only() {
        let dir = tmp_dir("sha");
        let path = dir.join("f.txt");
        std::fs::write(&path, b"abc").unwrap();
        let out = tool_checksum(&format!("sha256 {}", path.display())).await;
        assert_eq!(out, format!("SHA-256: {ABC_SHA256}"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn computes_md5_only() {
        let dir = tmp_dir("md5");
        let path = dir.join("f.txt");
        std::fs::write(&path, b"abc").unwrap();
        let out = tool_checksum(&format!("md5 {}", path.display())).await;
        assert_eq!(out, format!("MD5: {ABC_MD5}"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn accepts_sha_256_hyphen_variant() {
        let dir = tmp_dir("sha_hyphen");
        let path = dir.join("f.txt");
        std::fs::write(&path, b"abc").unwrap();
        let out = tool_checksum(&format!("SHA-256 {}", path.display())).await;
        assert_eq!(out, format!("SHA-256: {ABC_SHA256}"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn hashes_empty_file() {
        let dir = tmp_dir("empty");
        let path = dir.join("empty.txt");
        std::fs::write(&path, b"").unwrap();
        let out = tool_checksum(&path.display().to_string()).await;
        assert!(out.contains(EMPTY_SHA256));
        assert!(out.contains(EMPTY_MD5));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn hashes_large_file_via_chunks() {
        // Write a few MB so the streaming chunk loop exercises multiple reads.
        let dir = tmp_dir("big");
        let path = dir.join("big.bin");
        let payload = vec![0xA5u8; CHUNK_SIZE * 3 + 123];
        std::fs::write(&path, &payload).unwrap();

        // Compute expected hashes in one shot for comparison.
        let expected_sha = {
            let mut h = Sha256::new();
            h.update(&payload);
            hex_lower(&h.finalize())
        };
        let expected_md5 = {
            let mut h = Md5::new();
            h.update(&payload);
            hex_lower(&h.finalize())
        };

        let out = tool_checksum(&format!("both {}", path.display())).await;
        assert!(out.contains(&expected_sha), "got: {out}");
        assert!(out.contains(&expected_md5), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn missing_file_reports_error() {
        let out = tool_checksum("/tmp/aictl_checksum_nonexistent_xyz").await;
        assert!(out.starts_with("Error: file does not exist"), "got: {out}");
    }

    #[tokio::test]
    async fn directory_reports_error() {
        let dir = tmp_dir("is_dir");
        let out = tool_checksum(&dir.display().to_string()).await;
        assert!(out.contains("not a regular file"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn empty_input_reports_error() {
        let out = tool_checksum("").await;
        assert!(out.contains("Invalid input"), "got: {out}");
    }

    #[tokio::test]
    async fn path_with_sha256_prefix_in_name_still_works() {
        // A bare path whose basename starts with `sha256` must be treated as
        // the path itself, not as an algo prefix. The first word is the whole
        // path (no whitespace), so `parse_input` falls through to the default
        // `both` branch.
        let dir = tmp_dir("prefix_name");
        let path = dir.join("sha256_notes.txt");
        std::fs::write(&path, b"abc").unwrap();
        let out = tool_checksum(&path.display().to_string()).await;
        assert!(out.contains(ABC_SHA256), "got: {out}");
        assert!(out.contains(ABC_MD5), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hex_lower_formats_bytes() {
        assert_eq!(hex_lower(&[0x00, 0xff, 0xa5, 0x0f]), "00ffa50f");
    }
}
