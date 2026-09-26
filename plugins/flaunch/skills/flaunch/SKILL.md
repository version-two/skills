---
name: flaunch
version: 2.0.0
description: Expert guidance for flaunch CLI – the Rust-based build, version bump, upload, and notification tool for any project (built-in Flutter and Cargo types, or custom project types). Use when working with flaunch.yaml configs, version bumping, ReleaseHub uploads, Discord notifications, or custom project type definitions.
---

# FLaunch CLI Skill

Expert assistance for configuring and using flaunch - a blazing fast CLI tool for automating release workflows.

## When to Use This Skill

Invoke this skill when the user:
- Asks about flaunch commands or configuration
- Needs help setting up `flaunch.yaml` or `flaunch.local.yaml`
- Wants to generate config files for a project
- Wants to create custom project types
- Is troubleshooting version bumping or uploads
- Needs CI/CD integration examples
- Asks about ReleaseHub, SFTP upload, or Discord configuration

---

## Config Generation Workflow

When the user asks to generate flaunch config files, follow this workflow:

### Step 1: Detect Project Context

Before asking questions, try to auto-detect from the project directory:
- **pubspec.yaml** exists → Flutter project (check for `.fvm/` → use `fvm` type)
- **Cargo.toml** exists → Rust project (check for `installer/*.iss` → use `cargo-iss` type)
- **Android flavor directories** in `android/app/src/` → auto-detect flavor names
- **Existing flaunch.yaml** → offer to update rather than overwrite

### Step 2: Gather Required Information

Ask the user for info that can't be auto-detected. Use AskUserQuestion for structured choices:

1. **Project name** (auto-suggest from pubspec/Cargo.toml if available)
2. **Project type** (if not auto-detected): flutter, fvm, cargo, cargo-iss
3. **Bundle ID** (optional, e.g., `com.example.app`)
4. **Flavors** (auto-detect from android/app/src/ dirs, or ask)
5. **Upload method**: ReleaseHub, SFTP, or none
6. **Discord notifications**: yes/no
7. **Git integration**: yes/no

### Step 3: Generate flaunch.yaml

Write the main config file with:
- Public/non-sensitive settings
- Environment variable placeholders (`${VAR_NAME}`) for secrets
- Comments explaining each section

### Step 4: Generate flaunch.local.yaml

Write the local config with:
- Actual secret values (API keys, passwords, webhook URLs)
- Only the override fields, not the full config
- Remind user this file is auto-gitignored by flaunch

### Step 5: Validate

Run `flaunch validate` to verify the generated config.

---

## Complete YAML Schema Reference

Use this schema when generating configs. All fields with defaults can be omitted.

```yaml
# ═══════════════════════════════════════════════════════════════════════════════
# PROJECT (required)
# ═══════════════════════════════════════════════════════════════════════════════
project:
  name: "My App"                    # REQUIRED - Display name
  bundle_id: "com.example.app"      # Optional - used in output_naming {bundle_id}
  type: "flutter"                   # Default: "flutter". Options: flutter, fvm, cargo, cargo-iss
  output_naming: "{name}-{version}-{arch}-{flavor}.apk"  # Default shown
  # Placeholders: {name}, {bundle_id}, {version}, {semver}, {build}, {flavor}, {arch}

# ═══════════════════════════════════════════════════════════════════════════════
# FLAVORS (required, at least one)
# ═══════════════════════════════════════════════════════════════════════════════
flavors:
  <flavor_name>:                    # Key = flavor identifier (used in commands)
    display_name: "Human Name"      # Optional - for display/notifications
    single_apk: false               # Default: false. true = universal APK, false = split-per-abi
    build_commands:                  # REQUIRED - list of shell commands to run in order
      - "fvm flutter clean"
      - "fvm flutter pub get"
      - "fvm flutter build apk --flavor <name> --split-per-abi --release"
    output_paths:                   # REQUIRED - glob patterns for build outputs
      - "./build/app/outputs/apk/{flavor}/release/app-{flavor}-arm64-v8a-release.apk"
      - "./build/app/outputs/apk/{flavor}/release/app-{flavor}-armeabi-v7a-release.apk"
    env:                            # Optional - environment variables for build
      API_URL: "https://api.example.com"

# ═══════════════════════════════════════════════════════════════════════════════
# RELEASEHUB UPLOAD (optional - preferred over SFTP)
# ═══════════════════════════════════════════════════════════════════════════════
releasehub:
  api_url: "https://app.releasehub.dev"   # REQUIRED - ReleaseHub instance URL
  api_key: "${RELEASEHUB_API_KEY}"         # REQUIRED - API key (use env var or local config)
  channel: "stable"                        # Default: "stable". Options: stable, beta, nightly, etc.
  platform: "android"                      # Optional - e.g., android, windows, macos, universal
  chunk_size: 10485760                     # Default: 10MB (10485760 bytes)
  create_as_draft: false                   # Default: false
  changelog_template: ""                   # Optional - can include {commits} placeholder

# ═══════════════════════════════════════════════════════════════════════════════
# SFTP UPLOAD (optional - legacy alternative to ReleaseHub)
# ═══════════════════════════════════════════════════════════════════════════════
upload:
  host: "your-server.com"          # REQUIRED
  port: 22                         # Default: 22
  username: "deploy"               # REQUIRED
  password: "${SFTP_PASSWORD}"     # Use env var or local config
  # private_key: "~/.ssh/id_ed25519"  # Alternative to password
  remote_dir: "."                  # Default: "."

# ═══════════════════════════════════════════════════════════════════════════════
# DISCORD NOTIFICATIONS (optional)
# ═══════════════════════════════════════════════════════════════════════════════
discord:
  webhooks:                        # REQUIRED - list of webhook URLs
    - "${DISCORD_WEBHOOK_URL}"
  embed_color: 3066993             # Default: 3066993 (green #2ECC71). Decimal, not hex!
  embed_title: "Build Uploaded!"   # Default: "Build Uploaded!"
  include_commits: false           # Default: false. true = show git commits since last build

# ═══════════════════════════════════════════════════════════════════════════════
# GIT INTEGRATION (optional)
# ═══════════════════════════════════════════════════════════════════════════════
git:
  enabled: true                    # Default: true
  build_cache: ".flaunch_cache"    # Default: ".flaunch_cache" - tracks last build commit
  auto_commit_bump: false          # Default: false
  bump_commit_message: "chore: bump version to {version}"  # {version} placeholder
```

---

## Project Type Templates

### Flutter (with FVM)

```yaml
project:
  name: "{project_name}"
  type: "fvm"
  output_naming: "{name}-{version}-{arch}-{flavor}.apk"

flavors:
  dev:
    display_name: "Development"
    single_apk: false
    build_commands:
      - "fvm flutter clean"
      - "fvm flutter pub get"
      - "fvm flutter build apk --flavor dev --split-per-abi --release"
    output_paths:
      - "./build/app/outputs/apk/{flavor}/release/app-{flavor}-arm64-v8a-release.apk"
      - "./build/app/outputs/apk/{flavor}/release/app-{flavor}-armeabi-v7a-release.apk"

  prod:
    display_name: "Production"
    single_apk: true
    build_commands:
      - "fvm flutter clean"
      - "fvm flutter pub get"
      - "fvm flutter build apk --flavor prod --release"
    output_paths:
      - "./build/app/outputs/apk/{flavor}/release/app-{flavor}-release.apk"
```

### Flutter (without FVM)

Same as above but replace `fvm flutter` with `flutter` in build_commands.

### Cargo (Rust)

```yaml
project:
  name: "{project_name}"
  type: "cargo"
  output_naming: "{name}-{semver}-{arch}"

flavors:
  release:
    display_name: "Release"
    single_apk: true
    build_commands:
      - "cargo build --release"
    output_paths:
      - "target/release/{name}.exe"   # Windows
      # - "target/release/{name}"     # Linux/macOS
```

### Cargo + Inno Setup (Rust + Windows Installer)

```yaml
project:
  name: "{project_name}"
  type: "cargo-iss"
  output_naming: "{name}-{semver}-{arch}"

flavors:
  release:
    display_name: "Release"
    single_apk: false
    build_commands:
      - "powershell -ExecutionPolicy Bypass -File build-installer.ps1"
    output_paths:
      - "dist/{name}-*-setup.exe"
```

---

## flaunch.local.yaml Template

This file holds secrets. Only include overrides for sensitive fields:

```yaml
# flaunch.local.yaml - SECRET OVERRIDES (auto-gitignored)
# Only override fields that contain sensitive data

# For ReleaseHub uploads:
releasehub:
  api_key: "rh_live_actual_key_here"

# For SFTP uploads:
upload:
  password: "actual-sftp-password"

# For Discord notifications:
discord:
  webhooks:
    - "https://discord.com/api/webhooks/123456789/actual-token-here"
```

---

## Common Commands

```bash
flaunch                     # Interactive mode
flaunch production          # Run full pipeline for 'production' flavor
flaunch production -c beta  # Upload to 'beta' channel
flaunch production -t minor # Minor version bump

flaunch bump                # Bump version (build number)
flaunch bump -t patch       # Bump patch version
flaunch bump -t minor -m    # Minor bump + auto-commit

flaunch build -f prod       # Build only
flaunch upload -f prod      # Upload only
flaunch sync -c stable      # Sync version from ReleaseHub

flaunch version             # Show current version
flaunch flavors             # List available flavors
flaunch validate            # Check config
flaunch init                # Create new config (wizard)
```

## Version Bump Types

| Type | Example | Description |
|------|---------|-------------|
| `build` | 1.2.3+45 → 1.2.3+46 | Increment build number (Flutter default) |
| `patch` | 1.2.3 → 1.2.4 | Bug fixes (Cargo default) |
| `minor` | 1.2.3 → 1.3.0 | New features |
| `major` | 1.2.3 → 2.0.0 | Breaking changes |

## Project Types

| Type | Version File | Version Format | Default Bump |
|------|--------------|----------------|--------------|
| `flutter` | pubspec.yaml | X.Y.Z+B | build |
| `fvm` | pubspec.yaml | X.Y.Z+B | build |
| `cargo` | Cargo.toml | X.Y.Z | patch |
| `cargo-iss` | Cargo.toml + *.iss | X.Y.Z | patch |

## Output Naming Placeholders

| Placeholder | Example | Description |
|-------------|---------|-------------|
| `{name}` | MyApp | Project name |
| `{bundle_id}` | com.example.app | Bundle ID |
| `{version}` | 1.2.3+45 | Full version |
| `{semver}` | 1.2.3 | Semantic version only |
| `{build}` | 45 | Build number only |
| `{flavor}` | production | Build flavor |
| `{arch}` | arm64-v8a | Architecture |

## Best Practices

1. **Store secrets in `flaunch.local.yaml`** - Never commit API keys or passwords
2. **Use `fvm` type** for Flutter projects that use FVM version manager
3. **Use environment variables in CI/CD** - Reference with `${VAR_NAME}` syntax
4. **Run `flaunch validate`** after generating configs to verify correctness
5. **Split-per-abi** (`single_apk: false`) for dev builds, universal for prod
6. **Include `fvm flutter clean` + `fvm flutter pub get`** in build_commands for clean builds
