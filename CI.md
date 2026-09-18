# CI/CD and Development Workflow

This document describes the CI/CD setup, linting rules, and development workflow for rmpd.

## Table of Contents

- [Continuous Integration](#continuous-integration)
- [Linting and Formatting](#linting-and-formatting)
- [Security Auditing](#security-auditing)
- [Dependency Management](#dependency-management)
- [Local Development](#local-development)

## Continuous Integration

### GitHub Actions Workflows

#### Main CI Workflow (`.github/workflows/ci.yml`)

Runs on push to `main`, on pull requests, and via `workflow_dispatch`.

**Jobs:**

1. **Check** - Fast feedback loop
   - `cargo fmt --all -- --check`
   - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
   - `cargo doc --workspace --no-deps --all-features` (with `RUSTDOCFLAGS: -D warnings`)

2. **Test Suite** - Comprehensive testing
   - Matrix: Ubuntu + macOS × stable + nightly Rust
   - `cargo test --workspace --all-features -- --test-threads=1`
   - `cargo test --workspace --doc --all-features`

3. **Compatibility Tests** - Regression suites that guard cross-cutting behavior
   - State persistence: `cargo test --package rmpd-protocol --lib statefile` plus the
     `statefile_integration` and `state_resume_behavior` integration tests
   - Database compatibility: `cargo test --package rmpd-library --test compatibility_suite`
   - Decoder validation: `cargo test --package rmpd-player --test decoder_tests`, `edge_cases`,
     and `format_specific_tests`

4. **Coverage** - Code coverage reporting
   - Uses `cargo-llvm-cov` to produce `lcov.info`
   - Uploads to Codecov (requires `CODECOV_TOKEN` secret; upload failure does not fail the job)

5. **Lint Dependencies** - Unused dependency detection
   - Uses `cargo-machete` to find unused dependencies

6. **Build** - Multi-platform release builds
   - Targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`
     (3 targets - no x86_64 macOS build)
   - ARM64 Linux is cross-compiled from an Ubuntu runner
   - Artifacts uploaded per target

There is no MSRV job. See [Minimum Supported Rust Version](#minimum-supported-rust-version).

#### Lint Workflow (`.github/workflows/lint.yml`)

Runs on pull requests and pushes that touch `.github/workflows/**`, `**.toml`, `**.yml`/`**.yaml`.

**Jobs:**

1. **GitHub Actions Linting** - `reviewdog/action-actionlint@v1`
2. **YAML Linting** - `yamllint -d relaxed` over every `*.yml`/`*.yaml` file
3. **Validate Configuration Files** - parses `.github/renovate.json` as JSON and
   `clippy.toml`, `rustfmt.toml`, `.cargo/config.toml` as TOML
4. **Validate Renovate Config** - `suzuki-shunsuke/github-action-renovate-config-validator@v1.1.0`
   against `.github/renovate.json`

#### Security Workflow (`.github/workflows/security.yml`)

Runs on push/PR when `**/Cargo.toml` or `**/Cargo.lock` change, daily at 00:00 UTC, and via
`workflow_dispatch`.

**Jobs:**

1. **Security Audit** - `cargo audit --deny warnings` (CVE scanning via `cargo-audit`)
2. **Cargo Deny** - `cargo deny check` (licenses, advisories, bans, sources; see `deny.toml`)
3. **Supply Chain Security** - `cargo vet --locked`, wrapped in `continue-on-error: true` so a
   failure does not block the workflow

#### Release Workflow (`.github/workflows/release.yml`)

Triggers on tags matching `[0-9]+.[0-9]+.[0-9]+` (no `v` prefix). Builds the same 3 targets as
the CI `build` job, packages each as `rmpd-<tag>-<target>.tar.gz`, and creates a GitHub Release
whose body is auto-generated from `git log` between the previous tag and the new one.

#### Renovate Workflow (`.github/workflows/renovate.yml`)

Runs every Monday at 05:00 UTC (and via `workflow_dispatch`), invoking
`renovatebot/github-action` with `.github/renovate.json`.

### Required Secrets

Configure these in GitHub repository settings:

- `CODECOV_TOKEN` - Token for uploading coverage to Codecov (optional; upload failures are
  non-fatal)
- `RENOVATE_TOKEN` - Token used by the Renovate workflow to open PRs

## Linting and Formatting

### Rustfmt (`rustfmt.toml`)

```bash
# Check formatting
cargo fmt --all -- --check

# Apply formatting
cargo fmt --all
```

**Key settings (stable options only - see the file header):**
- Max width: 100 characters
- `reorder_imports` / `reorder_modules`: alphabetically reorder import statements and module
  declarations (grouping imports by `std` → external → `crate` needs rustfmt's unstable
  `group_imports`, which is not configured)
- `force_explicit_abi`, `use_field_init_shorthand`, `use_try_shorthand`: on
- `match_block_trailing_comma`: off; `match_arm_leading_pipes`: never
- Per-construct width heuristics tuned individually (`fn_call_width` 60, `struct_lit_width` 18,
  `chain_width` 60, `array_width` 60)

### Clippy (`clippy.toml` + `.cargo/config.toml`)

Clippy is driven by rustflags in `.cargo/config.toml` under `[target.'cfg(all())']`, which apply
to every build including `cargo clippy`:

- `-W clippy::all` - the only lint group enabled by default (`clippy::pedantic`, `clippy::nursery`
  and `clippy::cargo` are **not** turned on anywhere in the workspace)
- Explicit allows: `too_many_arguments`, `type_complexity`, `empty_line_after_doc_comments`,
  `redundant_field_names`, `unnecessary_cast`, `same_item_push`
- Each crate's `lib.rs` additionally has `#![allow(clippy::cargo_common_metadata)]`

`clippy.toml` sets thresholds and denylists, not lint groups:
- Thresholds: cognitive complexity 30, type complexity 250, single-char binding names 4,
  too-many-arguments 7, too-many-lines 100, array-size 512000
- `missing-docs-in-crate-items = true`
- `disallowed-methods`: `std::env::set_var`, `std::process::exit`, `std::panic::catch_unwind`
- `disallowed-types`: `std::collections::LinkedList`, `std::collections::BTreeMap`
- `doc-valid-idents`: MPD, DSD, PCM, ALSA, PulseAudio, PipeWire, ReplayGain, SQLite, HTTP, TCP,
  UDP, JSON, TOML, UTF-8

There is no `unwrap()`/`expect()`/`panic!()`/`print!()` restriction lint enabled anywhere in the
workspace - avoid those in new code as a convention, but clippy will not flag them today.

```bash
# Run clippy with the project's rustflags
cargo lint  # alias from .cargo/config.toml

# Or manually (matches the `check` job in ci.yml)
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

### Custom Aliases

Defined in `.cargo/config.toml`:

```bash
cargo lint          # clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt-check     # fmt --all -- --check
cargo test-all      # test --workspace --all-features
cargo doc-check     # doc --workspace --no-deps --all-features
```

## Security Auditing

### Cargo Audit

Scans dependencies for known security vulnerabilities. This is what the `Security Audit` job
in `security.yml` runs:

```bash
# Install
cargo install cargo-audit

# Run audit
cargo audit

# Deny warnings (matches CI)
cargo audit --deny warnings
```

### Cargo Deny (`deny.toml`)

Multi-purpose dependency checker; `cargo deny check` runs all four checks below.

**Checks:**

1. **Advisories** - Security vulnerabilities from RustSec. Two are explicitly ignored:
   `RUSTSEC-2024-0014` (`generational-arena`, pulled in by tantivy, no maintained alternative)
   and `RUSTSEC-2024-0436` (`paste`, feature-complete proc-macro).
2. **Licenses** - Only these are allowed: MIT, Apache-2.0, `Apache-2.0 WITH LLVM-exception`,
   BSD-2-Clause, BSD-3-Clause, ISC, Unicode-DFS-2016, Unicode-3.0, Zlib, 0BSD, CC0-1.0,
   Unlicense, MPL-2.0. `aws-lc-sys` has an explicit exception for its OpenSSL license.
3. **Bans** - Denies `openssl < 0.10` and `rustls < 0.21`; duplicate versions warn instead of
   fail.
4. **Sources** - Only `crates.io` is an allowed registry. Two git sources are allowed, both
   forks this workspace depends on directly: `https://github.com/M0Rf30/Symphonia` and
   `https://github.com/RustAudio/cpal`. There is no `lofty-rs` entry - `lofty` was removed from
   the dependency tree (see PR #16); all tag/artwork/metadata reading now goes through Symphonia.

```bash
# Install
cargo install cargo-deny

# Run all checks
cargo deny check

# Run specific check
cargo deny check advisories
cargo deny check licenses
cargo deny check bans
cargo deny check sources
```

### Cargo Vet

Supply chain security for dependencies. The `Supply Chain Security` job runs this in
non-blocking mode (`continue-on-error: true`), so a failure is visible but does not fail CI:

```bash
# Install
cargo install cargo-vet

# Initialize (first time)
cargo vet init

# Check dependencies
cargo vet --locked

# Certify a dependency after review
cargo vet certify <crate> <version>
```

## Dependency Management

### Renovate Bot (`.github/renovate.json` + `.github/workflows/renovate.yml`)

Automated dependency updates, run every Monday at 05:00 UTC by the `renovate.yml` workflow.

**Grouping (`packageRules`):**
- Non-major Rust dependency updates grouped together; majors get separate PRs
- GitHub Actions updates grouped and auto-merged for minor/patch
- Async runtime (`tokio`, `async-trait`, `futures`) grouped, no auto-merge
- Audio stack (`symphonia`, `cpal`, `rubato`) grouped, no auto-merge
- Database (`rusqlite`, `tantivy`) grouped, no auto-merge
- Dev-dependency minor/patch updates auto-merged
- Workspace dependencies get elevated PR priority

**Configuration:**
- Dependency dashboard in GitHub Issues
- Semantic commit messages, separate PRs per major version
- Vulnerability alerts enabled, lock file maintenance enabled
- Requires the `RENOVATE_TOKEN` secret

### Dependabot

**Disabled.** `.github/dependabot.yml` ships an empty `updates: []` list specifically to keep
Dependabot from acting, in favor of Renovate.

## Local Development

### Prerequisites

Install required system dependencies (matches what the CI jobs install):

**Ubuntu/Debian:**
```bash
sudo apt-get install libasound2-dev pkg-config libpipewire-0.3-dev libspa-0.2-dev libjack-dev
```

**macOS** (only needed by the `test` job's matrix leg):
```bash
brew install jack berkeley-db@5
```

### Development Tools

Install recommended Rust tools:

```bash
# Core tools (included in CI)
rustup component add rustfmt clippy

# Additional tools used by CI jobs
cargo install cargo-audit        # Security Audit job
cargo install cargo-deny         # Cargo Deny job
cargo install cargo-machete      # Lint Dependencies job
cargo install cargo-llvm-cov     # Coverage job
cargo install cargo-vet          # Supply Chain Security job

# Local-only convenience tool (not used in CI)
cargo install cargo-watch
```

### Pre-commit Hooks

Create `.git/hooks/pre-commit`:

```bash
#!/bin/bash
set -e

echo "Running pre-commit checks..."

# Check formatting
echo "→ Checking formatting..."
cargo fmt --all -- --check

# Run clippy
echo "→ Running clippy..."
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run tests
echo "→ Running tests..."
cargo test --workspace --all-features

echo "✓ All checks passed!"
```

Make it executable:
```bash
chmod +x .git/hooks/pre-commit
```

### Development Workflow

1. **Create a branch:**
   ```bash
   git checkout -b feature/my-feature
   ```

2. **Make changes and test:**
   ```bash
   # Run tests continuously
   cargo watch -x test

   # Check formatting
   cargo fmt

   # Run lints
   cargo lint
   ```

3. **Before committing:**
   ```bash
   # Format code
   cargo fmt

   # Check all lints
   cargo clippy --workspace --all-targets --all-features

   # Run all tests
   cargo test-all

   # Check docs
   cargo doc-check
   ```

4. **Commit and push:**
   ```bash
   git add .
   git commit -m "feat: add my feature"
   git push origin feature/my-feature
   ```

5. **Create PR** - CI will run automatically

### Quick Commands

```bash
# Full local CI simulation
cargo fmt && \
cargo clippy --workspace --all-targets --all-features -- -D warnings && \
cargo test --workspace --all-features && \
cargo doc --workspace --no-deps --all-features

# Security audit
cargo audit && cargo deny check

# Find unused dependencies
cargo machete

# Generate coverage
cargo llvm-cov --workspace --all-features --html
```

## Troubleshooting

### Clippy Warnings

If clippy is too strict for a specific case, you can allow specific lints:

```rust
// At function level
#[allow(clippy::too_many_lines)]
fn my_function() {
    // ...
}

// At module level
#![allow(clippy::cargo_common_metadata)]
```

**Note:** Use sparingly and only when justified! If a lint fires repeatedly across the codebase,
consider adding it to the `-A` list in `.cargo/config.toml` instead of scattering `#[allow]`.

### Minimum Supported Rust Version

rmpd has no dedicated MSRV CI job. `Cargo.toml` pins `edition = "2024"`, which requires Rust
1.85 or newer - that is the real floor. `check`, `compatibility`, `coverage`, `lint-dependencies`
and `build` all use `dtolnay/rust-toolchain@stable` (whatever stable currently is); `test` runs
both `stable` and `nightly`. If a dependency bump needs a newer toolchain than CI's `stable`
resolves to, it will show up as a `check`/`build` failure - there's no MSRV knob to adjust.

### Cross-compilation Issues

For ARM64 builds on Ubuntu, the CI `build`/`release` jobs reconfigure apt for multiarch and
install cross toolchains; the minimum for a local build is:

```bash
# Install cross-compiler
sudo apt-get install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu

# Set linker (matches ci.yml)
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc

# Build
cargo build --release --target aarch64-unknown-linux-gnu
```

## Additional Resources

- [Rust RFC 1444 - Clippy](https://rust-lang.github.io/rfcs/1444-union.html)
- [Cargo Deny Book](https://embarkstudios.github.io/cargo-deny/)
- [Renovate Documentation](https://docs.renovatebot.com/)
- [GitHub Actions Documentation](https://docs.github.com/en/actions)
