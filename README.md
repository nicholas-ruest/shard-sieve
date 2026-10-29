# shard-sieve

`shard-sieve` is a small Rust CLI and library for validating uncompressed TAR dataset shards before training or ingestion. It streams archive payloads, groups WebDataset-style members by logical sample key, and emits a deterministic JSON manifest suitable for CI.

It catches:

- missing required modalities such as `.jpg` plus `.json`;
- duplicate member paths, duplicate modalities, and logical sample keys repeated across shards;
- empty payloads, malformed sample names, unsafe paths, and non-regular archive members;
- shard sample-count skew beyond a configured threshold;
- optional SHA-256 payload digests without extracting the archive.

## Install

```bash
cargo install --git https://github.com/nicholas-ruest/shard-sieve
```

Or build locally with stable Rust 1.85 or newer:

```bash
git clone https://github.com/nicholas-ruest/shard-sieve.git
cd shard-sieve
cargo build --release
```

## Use

Validate that every sample has image and metadata members, enforce 25% maximum shard skew, hash payloads, and write a JSON manifest:

```bash
shard-sieve data/shard-000.tar data/shard-001.tar \
  --require jpg,json \
  --max-skew 0.25 \
  --hashes \
  --format json > shard-manifest.json
```

Text output is the default:

```text
PASS: 2 archive(s), 2000 sample(s), 4000 member(s), 98311420 payload byte(s)
  data/shard-000.tar: 1000 sample(s), 2000 member(s)
  data/shard-001.tar: 1000 sample(s), 2000 member(s)
```

Exit codes are `0` for a valid dataset, `2` for a completed inspection with validation issues, and `1` for a fatal I/O or format error. JSON issue codes are stable snake-case values intended for automation.

## Sample identity

The final filename extension is the modality. The remaining path is the logical key: `train/0001.jpg` and `train/0001.json` belong to sample `train/0001`; `train/0001.caption.txt` belongs to `train/0001.caption`. Keys must be unique across the supplied shards.

## Verification

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo +1.85.0 check --all-features
```

## Scope and limitations

- Inputs are uncompressed TAR files. Pipe or decompress `.tar.gz`/`.tar.zst` files before inspection.
- `shard-sieve` validates archive structure and declared modalities; it does not decode images, audio, tensors, or JSON schemas.
- Sample-count skew is measured against the arithmetic mean. It does not infer class balance or data quality.
- SHA-256 digests provide integrity identifiers, not authenticity or authorization.

## License

MIT
