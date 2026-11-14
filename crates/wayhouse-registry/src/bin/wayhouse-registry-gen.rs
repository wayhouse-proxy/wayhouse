//! Generate or check a sniffer registry `index.json` (`verify-plugin` checks a plugin index). Used by the sniffers repo CI.
//!
//! `generate` reads one directory per sniffer version (`manifest.toml`, `<name>.wasm`,
//! optionally `<name>.wasm.minisig`), merges them into the previous index and writes
//! the result atomically; a failed run leaves the existing file untouched.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use wayhouse_registry::catalog::parse_categories;
use wayhouse_registry::{
    generate, parse_detail, parse_index, parse_manifest, parse_plugin_detail, parse_plugin_index,
    to_json, upgrade_v1, Artifact, GenerateOptions, Index, IndexError, PluginIndex,
};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Merge built sniffers into an index and write it.
    Generate {
        /// Registry name stored in the index.
        #[arg(long)]
        name: String,
        /// Existing index to merge into (older versions are kept). Defaults to --out when that exists.
        #[arg(long)]
        previous: Option<PathBuf>,
        /// Where to write the index (may equal --previous).
        #[arg(long)]
        out: PathBuf,
        /// Download URL prefix; artifacts are at `<base-url>/<name>-v<version>/<name>.wasm`.
        #[arg(long)]
        base_url: String,
        /// `categories.toml`; every manifest category must be on its list.
        #[arg(long)]
        categories: Option<PathBuf>,
        /// `YYYY-MM-DD` recorded as `updated` for what is published now (default: today, UTC).
        #[arg(long)]
        date: Option<String>,
        /// Where the registry publishes `stats.json` (https).
        #[arg(long)]
        stats_url: Option<String>,
        /// Write the index as a catalog and one `<name>.json` detail file per sniffer here.
        /// Without it every version is inline in the one index file.
        #[arg(long, requires = "details_base_url")]
        details_dir: Option<PathBuf>,
        /// URL prefix the detail files are served under (`<prefix>/<name>.json`).
        #[arg(long)]
        details_base_url: Option<String>,
        /// One directory per sniffer version to publish.
        dirs: Vec<PathBuf>,
    },
    /// Parse an index with the real validator and print a summary.
    Verify {
        index: PathBuf,
        /// Also check the detail files of a catalog index (`<dir>/<name>.json`).
        #[arg(long)]
        details_dir: Option<PathBuf>,
    },
    /// Parse a plugin index (`kind: plugin`) with the real client validator and print a summary.
    VerifyPlugin {
        index: PathBuf,
        /// Also check the detail files of a catalog index (`<dir>/<name>.json`).
        #[arg(long)]
        details_dir: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Generate {
            name,
            previous,
            out,
            base_url,
            categories,
            date,
            stats_url,
            details_dir,
            details_base_url,
            dirs,
        } => {
            // Regenerating into an existing index must still honour version immutability,
            // so an existing --out is the comparison source unless --previous says otherwise.
            let today = date.unwrap_or_else(utc_today);
            let previous = previous
                .or_else(|| out.exists().then(|| out.clone()))
                .map(|p| -> Result<_> {
                    let bytes =
                        std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
                    let index = match parse_index(&bytes) {
                        // A schema 1 file is carried forward, not refused: its versions are
                        // published and immutable.
                        Err(IndexError::UnsupportedSchema(1)) => {
                            eprintln!("upgrading {} from schema 1", p.display());
                            upgrade_v1(&bytes, &today)
                        }
                        other => other,
                    }
                    .with_context(|| format!("{} is not a valid index", p.display()))?;
                    resolve_details(index, details_dir.as_deref())
                })
                .transpose()?;
            let allowed_categories = categories
                .map(|p| -> Result<_> {
                    let text = std::fs::read_to_string(&p)
                        .with_context(|| format!("reading {}", p.display()))?;
                    parse_categories(&text).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))
                })
                .transpose()?;
            let opts = GenerateOptions {
                registry_name: name,
                today,
                allowed_categories,
                stats_url,
            };
            let artifacts = dirs
                .iter()
                .map(|d| load_artifact(d, &base_url))
                .collect::<Result<Vec<_>>>()?;
            let g = generate(&opts, &artifacts, previous.as_ref())?;
            let mut index = g.index.clone();
            let mut details = Vec::new();
            if let (Some(dir), Some(base)) = (&details_dir, &details_base_url) {
                for entry in &mut index.sniffers {
                    let url = format!("{}/{}.json", base.trim_end_matches('/'), entry.name);
                    let (catalog, detail) = entry.split(url);
                    let mut json = serde_json::to_string_pretty(&detail)?;
                    json.push('\n');
                    details.push((dir.join(format!("{}.json", entry.name)), json));
                    *entry = catalog;
                }
            }
            // Nothing is written until the whole result is known to be valid.
            index.validate()?;
            for (path, json) in &details {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_atomically(path, json.as_bytes())?;
            }
            write_atomically(&out, to_json(&index).as_bytes())?;
            for (name, version) in &g.added {
                println!("added {name} {version}");
            }
            if g.added.is_empty() {
                println!("no changes");
            }
        }
        Command::Verify { index, details_dir } => {
            let bytes =
                std::fs::read(&index).with_context(|| format!("reading {}", index.display()))?;
            let parsed =
                parse_index(&bytes).with_context(|| format!("{} is invalid", index.display()))?;
            let parsed = resolve_details(parsed, details_dir.as_deref())?;
            let versions: usize = parsed.sniffers.iter().map(|s| s.versions.len()).sum();
            println!(
                "ok: {} sniffers, {versions} versions",
                parsed.sniffers.len()
            );
        }
        Command::VerifyPlugin { index, details_dir } => {
            let bytes =
                std::fs::read(&index).with_context(|| format!("reading {}", index.display()))?;
            let parsed = parse_plugin_index(&bytes)
                .with_context(|| format!("{} is invalid", index.display()))?;
            let parsed = resolve_plugin_details(parsed, details_dir.as_deref())?;
            let versions: usize = parsed.plugins.iter().map(|p| p.versions.len()).sum();
            println!("ok: {} plugins, {versions} versions", parsed.plugins.len());
        }
    }
    Ok(())
}

/// Plugin counterpart of `resolve_details`.
fn resolve_plugin_details(mut index: PluginIndex, dir: Option<&Path>) -> Result<PluginIndex> {
    for entry in &mut index.plugins {
        if entry.detail_url.is_none() {
            continue;
        }
        let dir = dir.context("the index points at detail files; pass --details-dir")?;
        let path = dir.join(format!("{}.json", entry.name));
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        *entry =
            parse_plugin_detail(&bytes, entry).with_context(|| format!("{}", path.display()))?;
    }
    Ok(index)
}

/// Fill in the versions of every catalog entry from `<dir>/<name>.json`, checked the way a
/// client checks a downloaded detail file. Inline entries pass through.
fn resolve_details(mut index: Index, dir: Option<&Path>) -> Result<Index> {
    for entry in &mut index.sniffers {
        if entry.detail_url.is_none() {
            continue;
        }
        let dir = dir.context("the index points at detail files; pass --details-dir")?;
        let path = dir.join(format!("{}.json", entry.name));
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        *entry = parse_detail(&bytes, entry).with_context(|| format!("{}", path.display()))?;
    }
    Ok(index)
}

/// Today's UTC date as `YYYY-MM-DD` (days since the epoch to a civil date).
fn utc_today() -> String {
    civil_date(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() / 86_400) as i64,
    )
}

fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn load_artifact(dir: &Path, base_url: &str) -> Result<Artifact> {
    let text = std::fs::read_to_string(dir.join("manifest.toml"))
        .with_context(|| format!("reading {}/manifest.toml", dir.display()))?;
    let manifest = parse_manifest(&text).with_context(|| dir.display().to_string())?;
    let file = format!("{}.wasm", manifest.name);
    let bytes = std::fs::read(dir.join(&file))
        .with_context(|| format!("reading {}/{file}", dir.display()))?;
    let url = format!(
        "{}/{}-v{}/{file}",
        base_url.trim_end_matches('/'),
        manifest.name,
        manifest.version
    );
    let signature_url = dir
        .join(format!("{file}.minisig"))
        .exists()
        .then(|| format!("{url}.minisig"));
    Ok(Artifact {
        manifest,
        bytes,
        url,
        signature_url,
    })
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("index")
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn civil_dates() {
        for (days, want) in [
            (0, "1970-01-01"),
            (11_016, "2000-02-29"),
            (20_769, "2026-11-12"),
        ] {
            assert_eq!(super::civil_date(days), want);
        }
    }
}
