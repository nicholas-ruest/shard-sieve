//! Streaming validation for TAR-based machine-learning dataset shards.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};
use tar::EntryType;
use thiserror::Error;

/// Configuration for one inspection run.
#[derive(Debug, Clone)]
pub struct InspectOptions {
    /// Every logical sample must contain each extension in this set.
    pub required_extensions: BTreeSet<String>,
    /// Maximum sample-count deviation from the mean, expressed as a fraction.
    pub max_skew: f64,
    /// Hash each regular-file payload while streaming it.
    pub include_hashes: bool,
}

impl Default for InspectOptions {
    fn default() -> Self {
        Self {
            required_extensions: BTreeSet::new(),
            max_skew: 0.5,
            include_hashes: false,
        }
    }
}

/// A complete, serializable validation result.
#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub valid: bool,
    pub required_extensions: Vec<String>,
    pub max_skew: f64,
    pub totals: Totals,
    pub archives: Vec<ArchiveReport>,
    pub issues: Vec<Issue>,
}

/// Totals across all inspected archives.
#[derive(Debug, Default, Serialize)]
pub struct Totals {
    pub archives: usize,
    pub samples: usize,
    pub members: usize,
    pub payload_bytes: u64,
}

/// Metadata and logical samples found in one archive.
#[derive(Debug, Serialize)]
pub struct ArchiveReport {
    pub path: PathBuf,
    pub archive_bytes: u64,
    pub sample_count: usize,
    pub member_count: usize,
    pub payload_bytes: u64,
    pub samples: Vec<SampleReport>,
}

/// A WebDataset-style sample, grouped by member name without its final extension.
#[derive(Debug, Serialize)]
pub struct SampleReport {
    pub key: String,
    pub members: Vec<MemberReport>,
}

/// One regular-file archive member.
#[derive(Debug, Serialize)]
pub struct MemberReport {
    pub path: String,
    pub extension: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// A validation failure. Any issue makes the report invalid.
#[derive(Debug, Serialize)]
pub struct Issue {
    pub code: IssueCode,
    pub archive: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    pub message: String,
}

/// Stable issue codes suitable for CI consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCode {
    UnsafePath,
    UnsupportedMemberType,
    DuplicateMember,
    MissingExtension,
    DuplicateExtension,
    EmptyPayload,
    InvalidSampleName,
    CrossShardDuplicate,
    ShardSkew,
}

/// Fatal errors that prevent inspection from producing a report.
#[derive(Debug, Error)]
pub enum InspectError {
    #[error("cannot read archive {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot inspect archive {path}: {source}")]
    Archive {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("max skew must be a finite non-negative number, got {0}")]
    InvalidSkew(f64),
}

#[derive(Default)]
struct SampleBuilder {
    members: Vec<MemberReport>,
    extensions: BTreeSet<String>,
}

/// Inspect one or more uncompressed TAR archives.
pub fn inspect_archives(
    paths: &[PathBuf],
    options: &InspectOptions,
) -> Result<Report, InspectError> {
    if !options.max_skew.is_finite() || options.max_skew < 0.0 {
        return Err(InspectError::InvalidSkew(options.max_skew));
    }

    let mut archives = Vec::with_capacity(paths.len());
    let mut issues = Vec::new();
    let mut sample_origins: BTreeMap<String, PathBuf> = BTreeMap::new();

    for path in paths {
        let archive = inspect_archive(path, options, &mut issues)?;
        for sample in &archive.samples {
            if let Some(first) = sample_origins.get(&sample.key) {
                issues.push(Issue {
                    code: IssueCode::CrossShardDuplicate,
                    archive: path.clone(),
                    member: Some(sample.key.clone()),
                    message: format!(
                        "logical sample '{}' also appears in {}",
                        sample.key,
                        first.display()
                    ),
                });
            } else {
                sample_origins.insert(sample.key.clone(), path.clone());
            }
        }
        archives.push(archive);
    }

    add_skew_issues(&archives, options.max_skew, &mut issues);

    let totals = Totals {
        archives: archives.len(),
        samples: archives.iter().map(|archive| archive.sample_count).sum(),
        members: archives.iter().map(|archive| archive.member_count).sum(),
        payload_bytes: archives.iter().map(|archive| archive.payload_bytes).sum(),
    };

    Ok(Report {
        schema_version: 1,
        valid: issues.is_empty(),
        required_extensions: options.required_extensions.iter().cloned().collect(),
        max_skew: options.max_skew,
        totals,
        archives,
        issues,
    })
}

fn inspect_archive(
    path: &Path,
    options: &InspectOptions,
    issues: &mut Vec<Issue>,
) -> Result<ArchiveReport, InspectError> {
    let file = File::open(path).map_err(|source| InspectError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let archive_bytes = file
        .metadata()
        .map_err(|source| InspectError::Open {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let mut tar = tar::Archive::new(BufReader::new(file));
    let entries = tar.entries().map_err(|source| InspectError::Archive {
        path: path.to_path_buf(),
        source,
    })?;
    let mut seen_members = BTreeSet::new();
    let mut samples: BTreeMap<String, SampleBuilder> = BTreeMap::new();
    let mut payload_bytes = 0_u64;

    for entry in entries {
        let mut entry = entry.map_err(|source| InspectError::Archive {
            path: path.to_path_buf(),
            source,
        })?;
        let member_name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        let entry_type = entry.header().entry_type();

        if entry_type == EntryType::Directory {
            continue;
        }
        if entry_type != EntryType::Regular {
            issues.push(Issue {
                code: IssueCode::UnsupportedMemberType,
                archive: path.to_path_buf(),
                member: Some(member_name),
                message: "only regular files and directories are allowed".to_owned(),
            });
            continue;
        }
        if !is_safe_member_path(&member_name) {
            issues.push(Issue {
                code: IssueCode::UnsafePath,
                archive: path.to_path_buf(),
                member: Some(member_name.clone()),
                message: "member path is absolute, empty, or traverses a parent directory"
                    .to_owned(),
            });
        }
        if !seen_members.insert(member_name.clone()) {
            issues.push(Issue {
                code: IssueCode::DuplicateMember,
                archive: path.to_path_buf(),
                member: Some(member_name.clone()),
                message: "archive contains the same member path more than once".to_owned(),
            });
        }

        let Some((key, extension)) = sample_identity(&member_name) else {
            issues.push(Issue {
                code: IssueCode::InvalidSampleName,
                archive: path.to_path_buf(),
                member: Some(member_name),
                message: "regular-file member must have a sample key and extension".to_owned(),
            });
            continue;
        };
        let bytes = entry
            .header()
            .size()
            .map_err(|source| InspectError::Archive {
                path: path.to_path_buf(),
                source,
            })?;
        if bytes == 0 {
            issues.push(Issue {
                code: IssueCode::EmptyPayload,
                archive: path.to_path_buf(),
                member: Some(member_name.clone()),
                message: "regular-file payload is empty".to_owned(),
            });
        }
        payload_bytes = payload_bytes.saturating_add(bytes);

        let sha256 = if options.include_hashes {
            Some(
                hash_reader(&mut entry).map_err(|source| InspectError::Archive {
                    path: path.to_path_buf(),
                    source,
                })?,
            )
        } else {
            None
        };

        let sample = samples.entry(key.clone()).or_default();
        if !sample.extensions.insert(extension.clone()) {
            issues.push(Issue {
                code: IssueCode::DuplicateExtension,
                archive: path.to_path_buf(),
                member: Some(member_name.clone()),
                message: format!(
                    "logical sample '{key}' contains extension '{extension}' more than once"
                ),
            });
        }
        sample.members.push(MemberReport {
            path: member_name,
            extension,
            bytes,
            sha256,
        });
    }

    for (key, sample) in &samples {
        for extension in options.required_extensions.difference(&sample.extensions) {
            issues.push(Issue {
                code: IssueCode::MissingExtension,
                archive: path.to_path_buf(),
                member: Some(key.clone()),
                message: format!("logical sample '{key}' is missing required '.{extension}' data"),
            });
        }
    }

    let samples: Vec<SampleReport> = samples
        .into_iter()
        .map(|(key, mut sample)| {
            sample
                .members
                .sort_by(|left, right| left.path.cmp(&right.path));
            SampleReport {
                key,
                members: sample.members,
            }
        })
        .collect();
    let member_count = samples.iter().map(|sample| sample.members.len()).sum();

    Ok(ArchiveReport {
        path: path.to_path_buf(),
        archive_bytes,
        sample_count: samples.len(),
        member_count,
        payload_bytes,
        samples,
    })
}

fn is_safe_member_path(member: &str) -> bool {
    let path = Path::new(member);
    !member.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn sample_identity(member: &str) -> Option<(String, String)> {
    let path = Path::new(member);
    let extension = path.extension()?.to_str()?.trim().to_ascii_lowercase();
    let stem = path.file_stem()?.to_str()?.trim();
    if extension.is_empty() || stem.is_empty() {
        return None;
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let key = match parent {
        Some(parent) => parent.join(stem).to_string_lossy().into_owned(),
        None => stem.to_owned(),
    };
    Some((key, extension))
}

fn hash_reader(reader: &mut impl Read) -> Result<String, std::io::Error> {
    let mut hasher = Sha256::new();
    std::io::copy(reader, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

fn add_skew_issues(archives: &[ArchiveReport], max_skew: f64, issues: &mut Vec<Issue>) {
    if archives.len() < 2 {
        return;
    }
    let mean = archives
        .iter()
        .map(|archive| archive.sample_count as f64)
        .sum::<f64>()
        / archives.len() as f64;
    if mean == 0.0 {
        return;
    }
    for archive in archives {
        let skew = (archive.sample_count as f64 - mean).abs() / mean;
        if skew > max_skew {
            issues.push(Issue {
                code: IssueCode::ShardSkew,
                archive: archive.path.clone(),
                member: None,
                message: format!(
                    "sample-count skew {skew:.3} exceeds configured maximum {max_skew:.3} (mean {mean:.2})"
                ),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_key_from_final_extension() {
        assert_eq!(
            sample_identity("train/0001.caption.txt"),
            Some(("train/0001.caption".to_owned(), "txt".to_owned()))
        );
    }

    #[test]
    fn rejects_missing_key_or_extension() {
        assert_eq!(sample_identity("README"), None);
        assert_eq!(sample_identity(".json"), None);
    }

    #[test]
    fn rejects_absolute_and_parent_paths() {
        assert!(!is_safe_member_path("/tmp/example.json"));
        assert!(!is_safe_member_path("../example.json"));
        assert!(is_safe_member_path("train/example.json"));
    }

    #[test]
    fn rejects_non_finite_skew() {
        let options = InspectOptions {
            max_skew: f64::NAN,
            ..InspectOptions::default()
        };
        let error = inspect_archives(&[], &options).expect_err("NaN must fail");
        assert!(matches!(error, InspectError::InvalidSkew(value) if value.is_nan()));
    }
}
