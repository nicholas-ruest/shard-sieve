use clap::{Parser, ValueEnum};
use shard_sieve::{inspect_archives, InspectOptions, Report};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    /// Uncompressed TAR shards to inspect.
    #[arg(required = true)]
    archives: Vec<PathBuf>,

    /// Required sample extensions, repeated or comma-separated (for example: jpg,json).
    #[arg(long = "require", value_delimiter = ',')]
    required_extensions: Vec<String>,

    /// Maximum fractional deviation from mean samples per shard.
    #[arg(long, default_value_t = 0.5)]
    max_skew: f64,

    /// Stream SHA-256 hashes for every regular-file member.
    #[arg(long)]
    hashes: bool,

    /// Report format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let required_extensions = cli
        .required_extensions
        .into_iter()
        .map(|extension| {
            extension
                .trim()
                .trim_start_matches('.')
                .to_ascii_lowercase()
        })
        .filter(|extension| !extension.is_empty())
        .collect::<BTreeSet<_>>();
    let options = InspectOptions {
        required_extensions,
        max_skew: cli.max_skew,
        include_hashes: cli.hashes,
    };
    let report = match inspect_archives(&cli.archives, &options) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("shard-sieve: {error}");
            return ExitCode::FAILURE;
        }
    };

    match cli.format {
        OutputFormat::Text => print_text(&report),
        OutputFormat::Json => match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("shard-sieve: cannot serialize report: {error}");
                return ExitCode::FAILURE;
            }
        },
    }

    if report.valid {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}

fn print_text(report: &Report) {
    println!(
        "{}: {} archive(s), {} sample(s), {} member(s), {} payload byte(s)",
        if report.valid { "PASS" } else { "FAIL" },
        report.totals.archives,
        report.totals.samples,
        report.totals.members,
        report.totals.payload_bytes
    );
    for archive in &report.archives {
        println!(
            "  {}: {} sample(s), {} member(s)",
            archive.path.display(),
            archive.sample_count,
            archive.member_count
        );
    }
    for issue in &report.issues {
        let member = issue
            .member
            .as_deref()
            .map(|value| format!(" [{value}]"))
            .unwrap_or_default();
        println!(
            "  {:?}: {}{}: {}",
            issue.code,
            issue.archive.display(),
            member,
            issue.message
        );
    }
}
