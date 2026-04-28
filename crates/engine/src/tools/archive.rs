//! Create, extract, and list tar.gz / tgz / zip archives.
//!
//! Format of the tool input:
//!
//! ```text
//! create <format> <output-archive-path>
//! <input-path-1>
//! <input-path-2>
//! ...
//! ```
//!
//! where `<format>` is `tar.gz` (alias `tgz`) or `zip`. Directory inputs are
//! added recursively; relative paths are resolved against the current working
//! directory.
//!
//! ```text
//! extract <archive-path> <destination-directory>
//! ```
//!
//! The format is detected from the archive's extension (`.tar.gz`, `.tgz`,
//! `.tar`, `.zip`). The destination directory is created if it does not
//! exist.
//!
//! ```text
//! list <archive-path>
//! ```
//!
//! lists the entries inside an archive without extracting anything.
//!
//! # Security
//!
//! Archive extraction is a classic zip-slip / tar-slip attack surface: a
//! malicious entry can carry a path like `../../etc/passwd` and, if blindly
//! written out, escape the destination directory. This module rejects every
//! entry whose normalized path leaves the destination — absolute paths,
//! `..` components, and symlinks are all refused at extract time. The tool
//! dispatcher in [`crate::tools::execute_tool`] additionally runs each path
//! through [`crate::security::validate_tool`] so the CWD jail and blocked
//! path list still apply to the archive itself and to the extraction root.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use super::util::truncate_output;

pub(super) async fn tool_archive(input: &str) -> String {
    let input = input.trim().to_string();
    tokio::task::spawn_blocking(move || run(&input))
        .await
        .unwrap_or_else(|e| format!("Error running archive tool: {e}"))
}

fn run(input: &str) -> String {
    let (first_line, rest) = match input.split_once('\n') {
        Some((a, b)) => (a.trim(), b),
        None => (input.trim(), ""),
    };

    let mut parts = first_line.split_whitespace();
    let op = parts.next().unwrap_or("");

    match op {
        "create" => {
            let Some(format) = parts.next() else {
                return usage("create: missing format (tar.gz | tgz | zip)");
            };
            let Some(output) = parts.next() else {
                return usage("create: missing output archive path");
            };
            if parts.next().is_some() {
                return usage(
                    "create: first line must be `create <format> <output>` — put input paths on subsequent lines",
                );
            }
            let inputs: Vec<&str> = rest
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            if inputs.is_empty() {
                return usage(
                    "create: no input paths supplied — list one path per line after the header",
                );
            }
            create_archive(format, output, &inputs)
        }
        "extract" => {
            let Some(archive) = parts.next() else {
                return usage("extract: missing archive path");
            };
            let Some(dest) = parts.next() else {
                return usage("extract: missing destination directory");
            };
            if parts.next().is_some() {
                return usage("extract: unexpected extra argument on header line");
            }
            if !rest.trim().is_empty() {
                return usage("extract: expected no data after header line");
            }
            extract_archive(archive, dest)
        }
        "list" => {
            let Some(archive) = parts.next() else {
                return usage("list: missing archive path");
            };
            if parts.next().is_some() {
                return usage("list: unexpected extra argument on header line");
            }
            list_archive(archive)
        }
        "" => usage("missing operation"),
        other => usage(&format!("unknown operation '{other}'")),
    }
}

fn usage(msg: &str) -> String {
    format!(
        "Invalid input: {msg}\n\n\
         Usage:\n\
         create <tar.gz|tgz|zip> <output>\\n<path>\\n<path>...\n\
         extract <archive> <destination>\n\
         list <archive>"
    )
}

// --- Format detection ---

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    TarGz,
    Tar,
    Zip,
}

fn parse_create_format(s: &str) -> Option<Format> {
    match s.to_ascii_lowercase().as_str() {
        "tar.gz" | "tgz" | "targz" => Some(Format::TarGz),
        "tar" => Some(Format::Tar),
        "zip" => Some(Format::Zip),
        _ => None,
    }
}

#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn detect_format(path: &str) -> Option<Format> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        Some(Format::TarGz)
    } else if lower.ends_with(".tar") {
        Some(Format::Tar)
    } else if lower.ends_with(".zip") {
        Some(Format::Zip)
    } else {
        None
    }
}

// --- Create ---

fn create_archive(format: &str, output: &str, inputs: &[&str]) -> String {
    let Some(fmt) = parse_create_format(format) else {
        return format!(
            "Error: unsupported archive format '{format}' (expected tar.gz, tgz, or zip)"
        );
    };

    for input in inputs {
        if !Path::new(input).exists() {
            return format!("Error: input path does not exist: {input}");
        }
    }

    let result = match fmt {
        Format::TarGz => create_tar_gz(output, inputs),
        Format::Tar => create_tar(output, inputs),
        Format::Zip => create_zip(output, inputs),
    };

    match result {
        Ok(count) => {
            let bytes = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
            format!("Created {output} ({count} entries, {bytes} bytes)")
        }
        Err(e) => {
            let _ = std::fs::remove_file(output);
            format!("Error creating archive: {e}")
        }
    }
}

fn create_tar_gz(output: &str, inputs: &[&str]) -> std::io::Result<usize> {
    let file = File::create(output)?;
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(enc);
    let count = write_tar_entries(&mut builder, inputs)?;
    let enc = builder.into_inner()?;
    enc.finish()?;
    Ok(count)
}

fn create_tar(output: &str, inputs: &[&str]) -> std::io::Result<usize> {
    let file = File::create(output)?;
    let mut builder = tar::Builder::new(file);
    let count = write_tar_entries(&mut builder, inputs)?;
    builder.finish()?;
    Ok(count)
}

fn write_tar_entries<W: Write>(
    builder: &mut tar::Builder<W>,
    inputs: &[&str],
) -> std::io::Result<usize> {
    let mut count = 0usize;
    for input in inputs {
        let path = Path::new(input);
        let name = path
            .file_name()
            .map_or_else(|| PathBuf::from(input), PathBuf::from);
        if path.is_dir() {
            builder.append_dir_all(&name, path)?;
        } else {
            builder.append_path_with_name(path, &name)?;
        }
        count += 1;
    }
    Ok(count)
}

fn create_zip(output: &str, inputs: &[&str]) -> std::io::Result<usize> {
    let file = File::create(output)?;
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut count = 0usize;
    for input in inputs {
        let path = Path::new(input);
        let base = path
            .file_name()
            .map_or_else(|| PathBuf::from(input), PathBuf::from);
        if path.is_dir() {
            count += add_dir_to_zip(&mut writer, path, &base, options)?;
        } else {
            add_file_to_zip(&mut writer, path, &base, options)?;
            count += 1;
        }
    }
    writer.finish()?;
    Ok(count)
}

fn add_file_to_zip<W: Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    src: &Path,
    name: &Path,
    options: zip::write::SimpleFileOptions,
) -> std::io::Result<()> {
    let name_str = name
        .to_str()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "non-UTF-8 path"))?;
    writer
        .start_file(name_str, options)
        .map_err(std::io::Error::other)?;
    let mut f = File::open(src)?;
    std::io::copy(&mut f, writer)?;
    Ok(())
}

fn add_dir_to_zip<W: Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    src: &Path,
    base: &Path,
    options: zip::write::SimpleFileOptions,
) -> std::io::Result<usize> {
    let mut count = 0usize;
    let dir_name = format!("{}/", base.to_string_lossy());
    writer
        .add_directory(&dir_name, options)
        .map_err(std::io::Error::other)?;
    count += 1;

    for entry in walk_dir(src)? {
        let rel = entry.strip_prefix(src).unwrap_or(&entry);
        let name = base.join(rel);
        if entry.is_dir() {
            let d = format!("{}/", name.to_string_lossy());
            writer
                .add_directory(&d, options)
                .map_err(std::io::Error::other)?;
        } else if entry.is_file() {
            add_file_to_zip(writer, &entry, &name, options)?;
        }
        count += 1;
    }
    Ok(count)
}

fn walk_dir(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_symlink() {
                continue; // skip symlinks to avoid escaping the tree
            }
            if ft.is_dir() {
                stack.push(path.clone());
            }
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

// --- Extract ---

fn extract_archive(archive: &str, dest: &str) -> String {
    let Some(fmt) = detect_format(archive) else {
        return format!(
            "Error: unsupported archive extension in '{archive}' (expected .tar.gz, .tgz, .tar, or .zip)"
        );
    };

    if !Path::new(archive).exists() {
        return format!("Error: archive does not exist: {archive}");
    }

    let dest_path = PathBuf::from(dest);
    if let Err(e) = std::fs::create_dir_all(&dest_path) {
        return format!("Error creating destination directory: {e}");
    }

    let dest_canon = match dest_path.canonicalize() {
        Ok(p) => p,
        Err(e) => return format!("Error resolving destination: {e}"),
    };

    let result = match fmt {
        Format::TarGz => extract_tar(archive, &dest_canon, true),
        Format::Tar => extract_tar(archive, &dest_canon, false),
        Format::Zip => extract_zip(archive, &dest_canon),
    };

    match result {
        Ok(count) => format!("Extracted {count} entries from {archive} to {dest}"),
        Err(e) => format!("Error extracting archive: {e}"),
    }
}

fn extract_tar(archive: &str, dest: &Path, gzipped: bool) -> std::io::Result<usize> {
    let file = File::open(archive)?;
    let mut count = 0usize;

    if gzipped {
        let dec = flate2::read::GzDecoder::new(file);
        let mut ar = tar::Archive::new(dec);
        count += extract_tar_entries(&mut ar, dest)?;
    } else {
        let mut ar = tar::Archive::new(file);
        count += extract_tar_entries(&mut ar, dest)?;
    }

    Ok(count)
}

fn extract_tar_entries<R: Read>(
    archive: &mut tar::Archive<R>,
    dest: &Path,
) -> std::io::Result<usize> {
    let mut count = 0usize;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.into_owned();
        let safe_rel = match safe_relative_path(&entry_path) {
            Ok(p) => p,
            Err(e) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        };

        // Skip symlinks and hardlinks — they can also escape the jail even
        // when the entry path itself is clean.
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            continue;
        }

        let out_path = dest.join(&safe_rel);
        if !out_path.starts_with(dest) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("entry escapes destination: {}", entry_path.display()),
            ));
        }

        if kind.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else if kind.is_file() {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
        } else {
            // Unsupported entry type — skip silently.
            continue;
        }
        count += 1;
    }
    Ok(count)
}

fn extract_zip(archive: &str, dest: &Path) -> std::io::Result<usize> {
    let file = File::open(archive)?;
    let mut ar = zip::ZipArchive::new(file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut count = 0usize;

    for i in 0..ar.len() {
        let mut entry = ar
            .by_index(i)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let Some(raw_path) = entry.enclosed_name() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("entry has unsafe path: {}", entry.name()),
            ));
        };
        let safe_rel = match safe_relative_path(&raw_path) {
            Ok(p) => p,
            Err(e) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        };

        let out_path = dest.join(&safe_rel);
        if !out_path.starts_with(dest) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("entry escapes destination: {}", raw_path.display()),
            ));
        }

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
        }
        count += 1;
    }
    Ok(count)
}

/// Return a relative, traversal-free version of an archive entry path.
/// Rejects absolute paths, root-dir components, and any `..` segments.
fn safe_relative_path(p: &Path) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("entry contains '..' component: {}", p.display()));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("entry is absolute: {}", p.display()));
            }
        }
    }
    Ok(out)
}

// --- List ---

fn list_archive(archive: &str) -> String {
    let Some(fmt) = detect_format(archive) else {
        return format!(
            "Error: unsupported archive extension in '{archive}' (expected .tar.gz, .tgz, .tar, or .zip)"
        );
    };

    if !Path::new(archive).exists() {
        return format!("Error: archive does not exist: {archive}");
    }

    let names = match fmt {
        Format::TarGz => list_tar(archive, true),
        Format::Tar => list_tar(archive, false),
        Format::Zip => list_zip(archive),
    };

    match names {
        Ok(list) => {
            if list.is_empty() {
                return "(archive is empty)".to_string();
            }
            let mut out = String::new();
            for n in &list {
                let _ = writeln!(out, "{n}");
            }
            truncate_output(&mut out);
            out.trim_end().to_string()
        }
        Err(e) => format!("Error listing archive: {e}"),
    }
}

fn list_tar(archive: &str, gzipped: bool) -> std::io::Result<Vec<String>> {
    let file = File::open(archive)?;
    let mut out = Vec::new();
    if gzipped {
        let dec = flate2::read::GzDecoder::new(file);
        let mut ar = tar::Archive::new(dec);
        for entry in ar.entries()? {
            let entry = entry?;
            out.push(entry.path()?.display().to_string());
        }
    } else {
        let mut ar = tar::Archive::new(file);
        for entry in ar.entries()? {
            let entry = entry?;
            out.push(entry.path()?.display().to_string());
        }
    }
    Ok(out)
}

fn list_zip(archive: &str) -> std::io::Result<Vec<String>> {
    let file = File::open(archive)?;
    let mut ar = zip::ZipArchive::new(file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut out = Vec::new();
    for i in 0..ar.len() {
        let entry = ar
            .by_index(i)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        out.push(entry.name().to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aictl_archive_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn safe_relative_accepts_normal() {
        let p = safe_relative_path(Path::new("a/b/c.txt")).unwrap();
        assert_eq!(p, PathBuf::from("a/b/c.txt"));
    }

    #[test]
    fn safe_relative_rejects_absolute() {
        assert!(safe_relative_path(Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn safe_relative_rejects_parent_dir() {
        assert!(safe_relative_path(Path::new("a/../../etc")).is_err());
    }

    #[test]
    fn detect_format_extensions() {
        assert!(matches!(detect_format("x.tar.gz"), Some(Format::TarGz)));
        assert!(matches!(detect_format("x.tgz"), Some(Format::TarGz)));
        assert!(matches!(detect_format("x.tar"), Some(Format::Tar)));
        assert!(matches!(detect_format("x.zip"), Some(Format::Zip)));
        assert!(detect_format("x.txt").is_none());
    }

    #[test]
    fn parse_create_format_accepts_known() {
        assert!(matches!(parse_create_format("tar.gz"), Some(Format::TarGz)));
        assert!(matches!(parse_create_format("tgz"), Some(Format::TarGz)));
        assert!(matches!(parse_create_format("ZIP"), Some(Format::Zip)));
        assert!(parse_create_format("rar").is_none());
    }

    #[tokio::test]
    async fn usage_on_missing_op() {
        let out = tool_archive("").await;
        assert!(out.contains("Invalid input"));
    }

    #[tokio::test]
    async fn create_and_extract_tar_gz_roundtrip() {
        let dir = tmp_dir("tgz_roundtrip");
        let src = dir.join("src.txt");
        std::fs::write(&src, "hello tar.gz").unwrap();
        let archive = dir.join("out.tar.gz");

        let create_in = format!("create tar.gz {}\n{}", archive.display(), src.display());
        let out = tool_archive(&create_in).await;
        assert!(out.starts_with("Created"), "got: {out}");

        let dest = dir.join("extracted");
        let extract_in = format!("extract {} {}", archive.display(), dest.display());
        let out = tool_archive(&extract_in).await;
        assert!(out.starts_with("Extracted"), "got: {out}");

        let extracted = dest.join("src.txt");
        assert_eq!(std::fs::read_to_string(&extracted).unwrap(), "hello tar.gz");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn create_and_extract_zip_roundtrip() {
        let dir = tmp_dir("zip_roundtrip");
        let src = dir.join("src.txt");
        std::fs::write(&src, "hello zip").unwrap();
        let archive = dir.join("out.zip");

        let create_in = format!("create zip {}\n{}", archive.display(), src.display());
        let out = tool_archive(&create_in).await;
        assert!(out.starts_with("Created"), "got: {out}");

        let dest = dir.join("extracted");
        let extract_in = format!("extract {} {}", archive.display(), dest.display());
        let out = tool_archive(&extract_in).await;
        assert!(out.starts_with("Extracted"), "got: {out}");

        assert_eq!(
            std::fs::read_to_string(dest.join("src.txt")).unwrap(),
            "hello zip"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn create_directory_and_roundtrip_tar_gz() {
        let dir = tmp_dir("tgz_dir");
        let src_dir = dir.join("src");
        std::fs::create_dir_all(src_dir.join("nested")).unwrap();
        std::fs::write(src_dir.join("a.txt"), "A").unwrap();
        std::fs::write(src_dir.join("nested/b.txt"), "B").unwrap();
        let archive = dir.join("out.tgz");

        let create_in = format!("create tgz {}\n{}", archive.display(), src_dir.display());
        let out = tool_archive(&create_in).await;
        assert!(out.starts_with("Created"), "got: {out}");

        let dest = dir.join("extracted");
        let extract_in = format!("extract {} {}", archive.display(), dest.display());
        let out = tool_archive(&extract_in).await;
        assert!(out.starts_with("Extracted"), "got: {out}");

        assert_eq!(
            std::fs::read_to_string(dest.join("src/a.txt")).unwrap(),
            "A"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("src/nested/b.txt")).unwrap(),
            "B"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn list_shows_entries() {
        let dir = tmp_dir("list");
        let src = dir.join("a.txt");
        std::fs::write(&src, "x").unwrap();
        let archive = dir.join("out.zip");
        let create_in = format!("create zip {}\n{}", archive.display(), src.display());
        assert!(tool_archive(&create_in).await.starts_with("Created"));

        let list_in = format!("list {}", archive.display());
        let out = tool_archive(&list_in).await;
        assert!(out.contains("a.txt"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn extract_rejects_traversal_entry_zip() {
        let dir = tmp_dir("zip_slip");
        let archive = dir.join("evil.zip");

        // Build a zip whose entry name is "../evil.txt" by bypassing the
        // safer helpers. zip's SimpleFileOptions writer accepts any name
        // we pass, so this is a realistic attacker-controlled archive.
        {
            let f = File::create(&archive).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            w.start_file("../evil.txt", opts).unwrap();
            w.write_all(b"pwned").unwrap();
            w.finish().unwrap();
        }

        let dest = dir.join("dest");
        let extract_in = format!("extract {} {}", archive.display(), dest.display());
        let out = tool_archive(&extract_in).await;
        assert!(
            out.contains("Error"),
            "traversal entry must be rejected, got: {out}"
        );
        assert!(!dir.join("evil.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn create_missing_input_path() {
        let dir = tmp_dir("missing");
        let archive = dir.join("out.tar.gz");
        let create_in = format!(
            "create tar.gz {}\n{}",
            archive.display(),
            dir.join("does-not-exist").display()
        );
        let out = tool_archive(&create_in).await;
        assert!(out.starts_with("Error"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn extract_unknown_extension() {
        let dir = tmp_dir("ext");
        let archive = dir.join("weird.rar");
        std::fs::write(&archive, b"").unwrap();
        let out = tool_archive(&format!("extract {} {}", archive.display(), dir.display())).await;
        assert!(out.contains("unsupported archive extension"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
