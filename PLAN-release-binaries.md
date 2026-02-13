# Plan: GitHub Actions Binary Releases with macOS Code Signing

## Overview

Add a new GitHub Actions workflow that builds signed release binaries for `hallucinator-cli` and `hallucinator-tui` across Linux, macOS (Intel + Apple Silicon), and Windows. macOS binaries will be code-signed and notarized so users can run them without `xattr` workarounds.

---

## 1. Apple Developer Account Setup (Manual, one-time)

Before any CI work, you need:

1. **Enroll in the Apple Developer Program** at https://developer.apple.com/programs/ ($99/year)
2. **Create a "Developer ID Application" certificate** in Certificates, Identifiers & Profiles
   - This is the certificate type for distributing *outside* the App Store
   - Download the `.p12` file (you'll set a password when exporting)
3. **Create an app-specific password** for notarization at https://appleid.apple.com/account/manage
   - Under "Sign-In and Security" > "App-Specific Passwords"
   - This is used by `notarytool` to authenticate
4. **Note your Team ID** - visible in your Apple Developer account membership page

### Secrets to add to the GitHub repo

| Secret Name | Value |
|---|---|
| `APPLE_CERTIFICATE_P12` | Base64-encoded `.p12` certificate (`base64 -i cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | Password you set when exporting the `.p12` |
| `APPLE_ID` | Your Apple ID email |
| `APPLE_ID_PASSWORD` | The app-specific password (NOT your Apple ID password) |
| `APPLE_TEAM_ID` | Your 10-character Team ID |

---

## 2. New Workflow: `.github/workflows/release-binaries.yml`

### Trigger

```yaml
on:
  push:
    tags: ["v*"]
  workflow_dispatch:  # manual trigger for testing
```

Same tag pattern as `python-wheels.yml` so a single `git tag v0.1.0 && git push --tags` fires both workflows.

### Build Matrix

```yaml
strategy:
  fail-fast: false
  matrix:
    include:
      - target: x86_64-unknown-linux-gnu
        os: ubuntu-latest
      - target: aarch64-unknown-linux-gnu
        os: ubuntu-latest        # cross-compile via cross or native ARM runner
      - target: x86_64-apple-darwin
        os: macos-13             # Intel runner
      - target: aarch64-apple-darwin
        os: macos-latest         # Apple Silicon runner (macos-14+)
      - target: x86_64-pc-windows-msvc
        os: windows-latest
```

**Note on aarch64-linux:** The `python-wheels.yml` has this excluded due to `ring` crate cross-compilation issues. Since `reqwest` uses `rustls-tls` (not native-tls), and `rustls` depends on `ring` (or `aws-lc-rs`), this may still be a problem. Two options:

- **Option A:** Use `cross` (Docker-based cross-compilation tool) which provides a full ARM toolchain. This usually handles `ring` fine because it compiles natively inside an ARM container via QEMU.
- **Option B:** Use GitHub's `ubuntu-24.04-arm` runners (now generally available) to compile natively on ARM. This is simpler and faster.
- **Option C:** Skip aarch64-linux for now, same as the Python wheels. Revisit later.

**Recommendation:** Option B (native ARM runner) if available in your plan, otherwise Option C to keep things simple initially.

### Build Steps (per target)

#### All platforms

```yaml
steps:
  - uses: actions/checkout@v4
    with:
      submodules: recursive
      fetch-depth: 500

  - uses: dtolnay/rust-toolchain@stable
    with:
      targets: ${{ matrix.target }}

  - name: Build release binaries
    run: |
      cargo build --release --target ${{ matrix.target }} \
        -p hallucinator-cli -p hallucinator-tui
    working-directory: hallucinator-rs
```

#### Linux-specific

```yaml
  - name: Install system deps (Linux)
    if: runner.os == 'Linux'
    run: sudo apt-get update && sudo apt-get -y install libfontconfig1-dev
```

#### macOS-specific: signing + notarization

```yaml
  - name: Import signing certificate
    if: runner.os == 'macOS'
    env:
      P12_BASE64: ${{ secrets.APPLE_CERTIFICATE_P12 }}
      P12_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
    run: |
      # Decode certificate
      echo "$P12_BASE64" | base64 --decode > certificate.p12

      # Create temporary keychain
      security create-keychain -p "" build.keychain
      security default-keychain -s build.keychain
      security unlock-keychain -p "" build.keychain

      # Import certificate
      security import certificate.p12 -k build.keychain \
        -P "$P12_PASSWORD" -T /usr/bin/codesign
      security set-key-partition-list -S apple-tool:,apple: \
        -s -k "" build.keychain

      # Clean up
      rm certificate.p12

  - name: Sign binaries
    if: runner.os == 'macOS'
    run: |
      IDENTITY="Developer ID Application: YOUR_NAME (${{ secrets.APPLE_TEAM_ID }})"
      for bin in hallucinator-cli hallucinator-tui; do
        codesign --sign "$IDENTITY" \
          --options runtime \
          --timestamp \
          "hallucinator-rs/target/${{ matrix.target }}/release/$bin"
      done

  - name: Notarize binaries
    if: runner.os == 'macOS'
    env:
      APPLE_ID: ${{ secrets.APPLE_ID }}
      APPLE_ID_PASSWORD: ${{ secrets.APPLE_ID_PASSWORD }}
      APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
    run: |
      for bin in hallucinator-cli hallucinator-tui; do
        BIN_PATH="hallucinator-rs/target/${{ matrix.target }}/release/$bin"

        # notarytool requires a zip/dmg/pkg - zip each binary
        zip "${bin}.zip" "$BIN_PATH"

        xcrun notarytool submit "${bin}.zip" \
          --apple-id "$APPLE_ID" \
          --password "$APPLE_ID_PASSWORD" \
          --team-id "$APPLE_TEAM_ID" \
          --wait

        rm "${bin}.zip"
      done
```

**Key details:**
- `--options runtime` enables the hardened runtime (required for notarization)
- `--timestamp` embeds a trusted timestamp (required for notarization)
- `--wait` blocks until Apple's servers return a result (typically 1-5 min)
- The zip is only for submission; the final artifact is the bare binary

### Packaging

After building (and signing on macOS), package the binaries:

```yaml
  - name: Package (Unix)
    if: runner.os != 'Windows'
    run: |
      mkdir -p staging
      for bin in hallucinator-cli hallucinator-tui; do
        cp "hallucinator-rs/target/${{ matrix.target }}/release/$bin" staging/
      done
      tar czf "hallucinator-${{ matrix.target }}.tar.gz" -C staging .

  - name: Package (Windows)
    if: runner.os == 'Windows'
    shell: bash
    run: |
      mkdir -p staging
      for bin in hallucinator-cli.exe hallucinator-tui.exe; do
        cp "hallucinator-rs/target/${{ matrix.target }}/release/$bin" staging/
      done
      cd staging && 7z a "../hallucinator-${{ matrix.target }}.zip" .

  - name: Upload artifact
    uses: actions/upload-artifact@v4
    with:
      name: binary-${{ matrix.target }}
      path: hallucinator-${{ matrix.target }}.*
```

### GitHub Release Job

A separate job that runs after all builds succeed:

```yaml
  release:
    name: Create GitHub Release
    needs: build
    runs-on: ubuntu-latest
    if: startsWith(github.ref, 'refs/tags/v')
    permissions:
      contents: write
    steps:
      - name: Download all artifacts
        uses: actions/download-artifact@v4
        with:
          path: artifacts
          merge-multiple: true

      - name: Create release
        uses: softprops/action-gh-release@v2
        with:
          files: artifacts/*
          generate_release_notes: true
          draft: true   # review before publishing
```

Using `draft: true` so you can review the release notes before making it public. Flip to `false` once you're confident in the pipeline.

---

## 3. Version Tagging Strategy

The workspace version is currently `0.1.0-alpha.1`. The release flow:

1. Bump version in `hallucinator-rs/Cargo.toml` workspace table
2. Commit: `git commit -m "release: v0.1.0-alpha.2"`
3. Tag: `git tag v0.1.0-alpha.2`
4. Push: `git push && git push --tags`
5. Both `release-binaries.yml` and `python-wheels.yml` fire automatically

Consider adding a `release.yml` or `release-please` config later if you want automated changelog generation and version bumping.

---

## 4. Optional: Homebrew Tap

Even with notarization, a Homebrew tap is the smoothest install experience for macOS users.

1. Create a new repo: `github.com/YOUR_ORG/homebrew-hallucinator`
2. Add a formula that downloads the macOS binary from the GitHub release
3. After each release, update the formula's URL and SHA256 (can be automated with a workflow that runs after the release is published)

Users install with:
```bash
brew tap YOUR_ORG/hallucinator
brew install hallucinator-cli hallucinator-tui
```

This can be a follow-up; the signed binaries are the priority.

---

## 5. Implementation Order

| Step | What | Depends On |
|------|------|------------|
| **1** | Apple Developer enrollment + certificate + app-specific password | Nothing (manual) |
| **2** | Add secrets to GitHub repo settings | Step 1 |
| **3** | Write `release-binaries.yml` with Linux + Windows builds only (no signing) | Nothing |
| **4** | Test with `workflow_dispatch` - verify binaries work | Step 3 |
| **5** | Add macOS signing + notarization steps | Steps 2, 4 |
| **6** | Test full pipeline with a `v*-rc1` tag | Step 5 |
| **7** | Add GitHub Release creation job | Step 6 |
| **8** | First real release | Step 7 |
| **9** | (Optional) Homebrew tap | Step 8 |

Steps 1-2 are manual/account setup. Steps 3-7 are the CI implementation work, and can be built incrementally - start with the basic build matrix, get it green, then layer on signing.

---

## 6. Known Issues / Gotchas

- **`ring` crate on aarch64-linux cross-compilation:** Already a known issue from `python-wheels.yml`. Native ARM runners are the cleanest fix.
- **`mupdf` system deps:** Needs `libfontconfig1-dev` on Linux. The `manylinux` containers in the Python workflow use `fontconfig-devel` (dnf). For the binary workflow on `ubuntu-latest`, it's `libfontconfig1-dev` (apt).
- **macOS runner selection:** `macos-13` = Intel, `macos-latest` (currently 14) = Apple Silicon. Use the right runner per target to avoid cross-compilation complexity.
- **Hardened runtime entitlements:** If the binaries need network access or other capabilities that the hardened runtime restricts, you may need an entitlements plist. For a CLI tool that just does HTTP requests, the default hardened runtime should be fine - network client access is allowed by default.
- **Certificate expiration:** Developer ID certificates last 5 years. Set a calendar reminder.
- **Notarization failures:** If notarization fails, `notarytool log <submission-id>` shows the exact reason. Common causes: missing timestamp, missing hardened runtime, or linking against a disallowed framework (unlikely for a Rust CLI).
- **Windows signing:** Not covered in this plan. Windows SmartScreen warnings for unsigned `.exe` files are less aggressive than macOS Gatekeeper. Can be added later with a code signing certificate from a CA like DigiCert (~$200-400/year) or via Azure Trusted Signing.
