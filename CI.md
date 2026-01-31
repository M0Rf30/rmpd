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

Runs on every push to `main` and on pull requests:

**Jobs:**

1. **Check** - Fast feedback loop
   - Code formatting check (`cargo fmt`)
   - Clippy lints (`cargo clippy`)
   - Documentation build (`cargo doc`)

2. **Test Suite** - Comprehensive testing
   - Matrix: Ubuntu + macOS × stable + nightly Rust
   - Unit tests (`cargo test`)
   - Doc tests (`cargo test --doc`)

3. **Coverage** - Code coverage reporting
   - Uses `cargo-llvm-cov` for coverage generation
   - Uploads to Codecov (requires `CODECOV_TOKEN` secret)

4. **Lint Dependencies** - Unused dependency detection
   - Uses `cargo-machete` to find unused dependencies

5. **MSRV** - Minimum Supported Rust Version
   - Ensures compatibility with Rust 1.75.0+

6. **Build** - Multi-platform builds
   - Targets: x86_64/aarch64 for Linux and macOS
   - Cross-compilation for ARM64
   - Artifacts uploaded for each target

#### Security Workflow (`.github/workflows/security.yml`)

Runs daily and on dependency changes:

**Jobs:**

1. **Security Audit** - CVE scanning
   - Uses `cargo-audit` to check for known vulnerabilities
   - Fails on any security advisories

2. **Cargo Deny** - License and security compliance
   - Checks licenses, advisories, and bans
   - Configured via `deny.toml`

3. **Supply Chain** - Dependency verification
   - Uses `cargo-vet` for supply chain security
   - Currently runs in non-blocking mode

### Required Secrets

Configure these in GitHub repository settings:

- `CODECOV_TOKEN` - Token for uploading coverage to Codecov (optional)

## Linting and Formatting

### Rustfmt (`rustfmt.toml`)

Automatic code formatting with strict settings:

```bash
# Check formatting
cargo fmt --all -- --check

# Apply formatting
cargo fmt --all
```

**Key settings:**
- Max width: 100 characters
- Group and reorder imports by `std` → `external` → `crate`
- Force explicit ABI
- Use field init shorthand
- Trailing commas in vertical layouts

### Clippy (`clippy.toml` + `.cargo/config.toml`)

**Lint Groups Enabled:**
- `clippy::all` - All standard lints
- `clippy::pedantic` - Extra pedantic lints
- `clippy::nursery` - Experimental lints
- `clippy::cargo` - Cargo-specific lints

**Carefully Selected Restriction Lints:**
- No `unwrap()`, `expect()`, `panic!()`, `todo()`, `unimplemented!()`
- No `dbg!()` macro in production code
- No `print!()` or `println!()` (use `tracing` instead)
- Prevent common performance pitfalls

**Allowed Exceptions:**
- `missing_errors_doc` - Error docs not required everywhere
- `missing_panics_doc` - Panic docs not required everywhere
- `module_name_repetitions` - Allow repetition in module names
- Float casting lints - Too noisy for audio code

```bash
# Run clippy with all lints
cargo lint  # Uses alias from .cargo/config.toml

# Or manually
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

### Custom Aliases

Defined in `.cargo/config.toml`:

```bash
cargo lint          # Run clippy with strict settings
cargo fmt-check     # Check formatting without applying
cargo test-all      # Run all tests with all features
cargo doc-check     # Build docs with warnings as errors
```

## Security Auditing

### Cargo Audit

Scans dependencies for known security vulnerabilities:

```bash
# Install
cargo install cargo-audit

# Run audit
cargo audit

# Deny warnings
cargo audit --deny warnings
```

### Cargo Deny (`deny.toml`)

Multi-purpose dependency checker:

**Checks:**

1. **Advisories** - Security vulnerabilities from RustSec
2. **Licenses** - Only approved licenses allowed:
   - MIT, Apache-2.0, BSD-{2,3}-Clause, ISC
   - Zlib, 0BSD, CC0-1.0, Unlicense
3. **Bans** - Forbidden crates and old versions
4. **Sources** - Only trusted registries and git repos

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

Supply chain security for dependencies:

```bash
# Install
cargo install cargo-vet

# Initialize (first time)
cargo vet init

# Check dependencies
cargo vet

# Certify a dependency after review
cargo vet certify <crate> <version>
```

## Dependency Management

### Renovate Bot (`.github/renovate.json`)

Automated dependency updates with intelligent grouping:

**Features:**
- Weekly updates (Monday before 6am)
- Groups related dependencies together
- Auto-merges minor/patch updates for dev dependencies
- Separate PRs for major updates
- High priority for security updates
- Smart grouping:
  - Async runtime (tokio, futures)
  - Audio stack (symphonia, cpal, lofty)
  - Database (rusqlite, tantivy)
  - GitHub Actions

**Configuration:**
- Dependency dashboard in GitHub Issues
- Semantic commit messages
- Vulnerability alerts enabled
- Lock file maintenance enabled

### Dependabot

**Disabled** - Using Renovate instead for better control and features.

## Local Development

### Prerequisites

Install required system dependencies:

**Ubuntu/Debian:**
```bash
sudo apt-get install libasound2-dev pkg-config
```

**macOS:**
```bash
brew install pkg-config
```

### Development Tools

Install recommended Rust tools:

```bash
# Core tools (included in CI)
rustup component add rustfmt clippy

# Additional tools
cargo install cargo-audit        # Security auditing
cargo install cargo-deny         # License/security checking
cargo install cargo-machete      # Find unused dependencies
cargo install cargo-llvm-cov     # Code coverage
cargo install cargo-vet          # Supply chain security
cargo install cargo-watch        # Watch for changes
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
#[allow(clippy::unwrap_used)]
fn my_function() {
    // Can use unwrap here
}

// At module level
#![allow(clippy::missing_errors_doc)]

// Inline
let x = some_option.unwrap(); // #[allow(clippy::unwrap_used)]
```

**Note:** Use sparingly and only when justified!

### MSRV Issues

If a dependency requires a newer Rust version:

1. Update MSRV in `.github/workflows/ci.yml`
2. Document in README
3. Consider if the dependency is essential

### Cross-compilation Issues

For ARM64 builds on Ubuntu:

```bash
# Install cross-compiler
sudo apt-get install gcc-aarch64-linux-gnu

# Set linker
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc

# Build
cargo build --target aarch64-unknown-linux-gnu
```

## Additional Resources

- [Rust RFC 1444 - Clippy](https://rust-lang.github.io/rfcs/1444-union.html)
- [Cargo Deny Book](https://embarkstudios.github.io/cargo-deny/)
- [Renovate Documentation](https://docs.renovatebot.com/)
- [GitHub Actions Documentation](https://docs.github.com/en/actions)
