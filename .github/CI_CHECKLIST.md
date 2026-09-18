# CI/CD Setup Checklist

## ✅ Initial Setup (Completed)

- [x] GitHub Actions workflows created (`ci.yml`, `lint.yml`, `security.yml`, `renovate.yml`,
      `release.yml`)
- [x] Linting configuration files created (`clippy.toml`, `rustfmt.toml`, `.cargo/config.toml`)
- [x] Renovate configuration created (`.github/renovate.json`)
- [x] Dependabot explicitly disabled (`.github/dependabot.yml`)
- [x] Documentation written (see `CI.md`)

## 🔧 Repository Configuration (TODO)

### 1. GitHub Actions

- [ ] Push changes to main branch
- [ ] Verify workflows run successfully
- [ ] Check Actions tab for any failures

### 2. Secrets Configuration

- [ ] Add `CODECOV_TOKEN` (optional; coverage upload failure is non-fatal)
  - Sign up at https://codecov.io
  - Add repository
  - Copy token
  - Go to Settings → Secrets → Actions → New secret
- [ ] Add `RENOVATE_TOKEN` (required for the `renovate.yml` workflow to open PRs)

### 3. Renovate Bot

- [ ] Install Renovate from GitHub Marketplace
  - Visit: https://github.com/apps/renovate
  - Click "Install"
  - Select your repository
  - Approve permissions
- [ ] Wait for first PR (Renovate onboarding)
- [ ] Review and merge onboarding PR
- [ ] Check for dependency update PRs

### 4. Branch Protection

- [ ] Go to Settings → Branches
- [ ] Add rule for `main` branch, requiring these status checks (job names from `ci.yml` /
      `security.yml`):
  - [ ] Check
  - [ ] Test Suite (ubuntu-latest, stable)
  - [ ] Compatibility Tests
  - [ ] Security Audit
  - [ ] Cargo Deny
  - [ ] Require review before merge (recommended: 1 approver)
  - [ ] Require linear history
  - [ ] Include administrators (optional)

### 5. Issue Templates

- [ ] Verify issue templates work:
  - Go to Issues → New Issue
  - Check `bug_report.md` and `feature_request.md` appear

### 6. Badges (Optional)

Add to README.md:

```markdown
[![CI](https://github.com/M0Rf30/rmpd/actions/workflows/ci.yml/badge.svg)](https://github.com/M0Rf30/rmpd/actions/workflows/ci.yml)
[![Security](https://github.com/M0Rf30/rmpd/actions/workflows/security.yml/badge.svg)](https://github.com/M0Rf30/rmpd/actions/workflows/security.yml)
[![Lint](https://github.com/M0Rf30/rmpd/actions/workflows/lint.yml/badge.svg)](https://github.com/M0Rf30/rmpd/actions/workflows/lint.yml)
[![codecov](https://codecov.io/gh/M0Rf30/rmpd/branch/main/graph/badge.svg)](https://codecov.io/gh/M0Rf30/rmpd)
```

## 🧪 Local Testing

### Before First Push

Test locally to ensure CI will pass:

```bash
# Format code
cargo fmt --all

# Check formatting (dry run)
cargo fmt --all -- --check

# Run clippy (matches the Check job)
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run tests (matches the Test Suite job)
cargo test --workspace --all-features -- --test-threads=1

# Check docs (matches the Check job, RUSTDOCFLAGS="-D warnings")
cargo doc --workspace --no-deps --all-features

# Security audit (matches the Security workflow)
cargo install cargo-audit cargo-deny
cargo audit --deny warnings
cargo deny check

# Find unused deps (matches Lint Dependencies)
cargo install cargo-machete
cargo machete
```

### Pre-commit Hook (Recommended)

```bash
cat > .git/hooks/pre-commit << 'EOF'
#!/bin/bash
set -e

echo "🔍 Running pre-commit checks..."

echo "  → Checking formatting..."
cargo fmt --all -- --check

echo "  → Running clippy..."
cargo clippy --workspace --all-targets --all-features -- -D warnings

echo "  → Running tests..."
cargo test --workspace --all-features

echo "✅ All checks passed!"
EOF

chmod +x .git/hooks/pre-commit
```

## 📊 Monitoring

### After Setup

- [ ] Monitor first CI run
- [ ] Check for any security advisories
- [ ] Review first Renovate PRs
- [ ] Verify code coverage reports

### Regular Checks

- [ ] Review Renovate PRs weekly
- [ ] Check security workflow daily runs
- [ ] Monitor CI performance/costs

## 🔒 Security Best Practices

- [ ] Enable Dependabot alerts (Security → Code security → Dependabot) - note: update PRs stay
      disabled via `.github/dependabot.yml`, only alerts are relevant
- [ ] Enable secret scanning (Security → Code security → Secret scanning)
- [ ] Review security advisories regularly
- [ ] Keep dependencies up to date via Renovate

## 🎯 Success Criteria

Your CI/CD is working correctly when:

- ✅ All workflows run without errors
- ✅ Code formatting is enforced
- ✅ Clippy catches potential issues
- ✅ Tests pass on multiple platforms
- ✅ Security audits report no issues
- ✅ Renovate creates update PRs
- ✅ Coverage reports are generated

## 🆘 Troubleshooting

### CI Failing on Clippy

If too many clippy warnings, discuss adding the lint to the `-A` list in `.cargo/config.toml`
(under `[target.'cfg(all())']`) rather than scattering per-site `#[allow(...)]`.

### Build Failing on ARM64

Install cross-compilation tools:

```bash
sudo apt-get install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu
```

### Renovate Not Creating PRs

1. Check Renovate logs in the dependency dashboard issue
2. Verify `.github/renovate.json` is valid JSON
3. Verify the `RENOVATE_TOKEN` secret is set
4. Check repository settings allow app access

### Coverage Upload Failing

1. Verify `CODECOV_TOKEN` secret is set
2. Check Codecov.io repository is added
3. Token has correct permissions

## 📚 Additional Resources

- [GitHub Actions Documentation](https://docs.github.com/en/actions)
- [Clippy Lints](https://rust-lang.github.io/rust-clippy/master/)
- [Cargo Deny](https://embarkstudios.github.io/cargo-deny/)
- [Renovate Docs](https://docs.renovatebot.com/)
- [Rust Security](https://rustsec.org/)

## ✨ Optional Enhancements

Future improvements to consider:

- [ ] Add benchmarking (criterion.rs)
- [ ] Add mutation testing (cargo-mutants)
- [ ] Add fuzzing (cargo-fuzz)
- [ ] Automated releases (cargo-release) - note: `release.yml` already builds and publishes
      tagged releases; this item would be about automating the version bump/tag itself
- [ ] Changelog generation (git-cliff) - note: `release.yml` already generates release notes
      from `git log` between tags; this item would be about a persisted `CHANGELOG.md`
- [ ] Docker image builds
- [ ] Performance regression detection
