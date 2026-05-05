# Plan: macOS Developer ID signing & notarization for `aictl-desktop`

## Context

The desktop release workflow currently produces an **ad-hoc-signed** `.app` bundle (commit `084a4a9`, `.github/workflows/release.yml:303`):

```bash
codesign --force --deep --sign - "$app_dir"
```

Ad-hoc signing is enough for local builds and `xattr -d com.apple.quarantine` workflows, but anything downloaded from a browser (GitHub releases included) carries the quarantine flag, so on first launch macOS shows:

> Apple nie może zweryfikować, czy „aictl.app" nie zawiera szkodliwego oprogramowania, które może uszkodzić Maca lub naruszyć Twoją prywatność.
>
> ("Apple cannot verify that 'aictl.app' is free of malware that could damage your Mac or violate your privacy.")

The README documents the right-click-Open / System Settings → Privacy & Security workaround, but every user hits the dialog on first install. The fix is a real **Developer ID Application** signature plus **Apple notarization**, with the notarization ticket stapled into the `.app` and the DMG.

The release workflow's own comment already calls this out — *"Ad-hoc (`--sign -`) is enough for Gatekeeper-Lite + xattr quarantine clearance; full notarization still requires a Developer ID identity."* — so this plan operationalizes that follow-up.

## Goals & non-goals

**Goals**
- DMG downloads from GitHub Releases open without any Gatekeeper warning, no right-click trick, no `xattr` workaround.
- Signing + notarization run inside the existing `.github/workflows/release.yml` job — no manual steps for a release.
- Hardened runtime is enabled, with the minimum entitlements required for MLX / llama.cpp runtime codegen.
- The ticket is stapled so Gatekeeper validates offline (no network round-trip on launch).

**Non-goals**
- Apple Developer Program enrolment automation (one-time human step — $99/year individual membership).
- App Store distribution. We're targeting Developer ID + notarization for direct distribution from GitHub Releases, not Mac App Store review.
- Sparkle / Tauri auto-updater integration. The desktop plan covers that separately; this plan only ensures the artefact users *download* is trusted.
- Cross-platform signing (Windows Authenticode, Linux AppImage signing). macOS-only matches the current desktop scope.

## Prerequisites (one-time, manual)

1. **Apple Developer Program** — $99/year (apple.com/developer). Individual membership is sufficient; no need for the organization tier unless we want a company name on the cert.
2. **Developer ID Application certificate** — generated in Apple Developer portal → Certificates, Identifiers & Profiles. Download into login keychain. The identity string looks like `Developer ID Application: Piotr Wittchen (TEAMID)`.
3. **App-specific password** — generated at appleid.apple.com → Sign-In and Security → App-Specific Passwords. Used by `notarytool` for non-interactive auth.
4. **GitHub Actions secrets** — see §3.

## 1. Entitlements file

Create `crates/aictl-desktop/entitlements.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.cs.allow-jit</key><true/>
    <key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
    <key>com.apple.security.cs.disable-library-validation</key><true/>
</dict>
</plist>
```

Rationale:
- `allow-jit` + `allow-unsigned-executable-memory` — required because `mlx` and the `gguf` (llama.cpp) backends do runtime codegen / load JIT-compiled kernels. Without these, the hardened runtime kills the process on first inference.
- `disable-library-validation` — required because the bundle drops `mlx.metallib` into `Contents/MacOS/` after build (release.yml:282) and may load other unsigned libs from the same dir; library validation would refuse them.
- These entitlements are accepted by Apple notarization for direct-distribution apps; they would *not* be accepted on the Mac App Store.

If the desktop is built without `mlx` and `gguf` features in the future, drop `allow-jit` / `allow-unsigned-executable-memory` and keep only `disable-library-validation` (or remove all of them).

## 2. Replace the ad-hoc sign step

In `.github/workflows/release.yml`, the current step at lines 284–304:

```yaml
- name: Ad-hoc sign .app bundle
  ...
  run: |
    codesign --force --deep --sign - "$app_dir"
    codesign --verify --deep --strict --verbose=2 "$app_dir"
```

Becomes:

```yaml
- name: Import signing certificate
  shell: bash
  env:
    MACOS_CERTIFICATE: ${{ secrets.MACOS_CERTIFICATE }}
    MACOS_CERTIFICATE_PASSWORD: ${{ secrets.MACOS_CERTIFICATE_PASSWORD }}
    KEYCHAIN_PASSWORD: ${{ secrets.MACOS_KEYCHAIN_PASSWORD }}
  run: |
    echo "$MACOS_CERTIFICATE" | base64 -d > /tmp/cert.p12
    security create-keychain -p "$KEYCHAIN_PASSWORD" build.keychain
    security set-keychain-settings -lut 21600 build.keychain
    security unlock-keychain -p "$KEYCHAIN_PASSWORD" build.keychain
    security default-keychain -s build.keychain
    security import /tmp/cert.p12 -k build.keychain \
      -P "$MACOS_CERTIFICATE_PASSWORD" \
      -T /usr/bin/codesign -T /usr/bin/security
    security set-key-partition-list -S apple-tool:,apple: \
      -s -k "$KEYCHAIN_PASSWORD" build.keychain
    rm -f /tmp/cert.p12

- name: Sign .app bundle (Developer ID + hardened runtime)
  shell: bash
  env:
    SIGNING_IDENTITY: ${{ secrets.MACOS_SIGNING_IDENTITY }}
  run: |
    app_dir=$(find "target/${{ matrix.target }}/release/bundle/macos" -type d -name "*.app" -print -quit)
    if [ -z "$app_dir" ]; then
      echo "ERROR: .app not produced"
      exit 1
    fi
    codesign --force --deep --options runtime --timestamp \
      --entitlements crates/aictl-desktop/entitlements.plist \
      --sign "$SIGNING_IDENTITY" \
      "$app_dir"
    codesign --verify --deep --strict --verbose=2 "$app_dir"
```

Notes:
- `--options runtime` is the hardened runtime flag. Apple refuses to notarize without it.
- `--timestamp` requests a secure timestamp from Apple's TSA. Required for notarization.
- `--entitlements` points at the file from §1.
- The `mlx.metallib` injection step (release.yml:269–282) must still run **before** signing — signing seals the bundle contents; copying anything in afterwards invalidates the signature. The current ordering is already correct.

## 3. Notarize and staple

After the existing `Package DMG` step (release.yml:306–332) builds the DMG, add:

```yaml
- name: Sign DMG
  shell: bash
  env:
    SIGNING_IDENTITY: ${{ secrets.MACOS_SIGNING_IDENTITY }}
  run: |
    codesign --force --sign "$SIGNING_IDENTITY" --timestamp "${{ matrix.artifact }}.dmg"

- name: Notarize DMG
  shell: bash
  env:
    NOTARY_APPLE_ID: ${{ secrets.MACOS_NOTARY_APPLE_ID }}
    NOTARY_TEAM_ID: ${{ secrets.MACOS_NOTARY_TEAM_ID }}
    NOTARY_PASSWORD: ${{ secrets.MACOS_NOTARY_PASSWORD }}
  run: |
    xcrun notarytool submit "${{ matrix.artifact }}.dmg" \
      --apple-id "$NOTARY_APPLE_ID" \
      --team-id "$NOTARY_TEAM_ID" \
      --password "$NOTARY_PASSWORD" \
      --wait

- name: Staple ticket
  run: |
    xcrun stapler staple "${{ matrix.artifact }}.dmg"
    xcrun stapler validate "${{ matrix.artifact }}.dmg"
```

`notarytool submit ... --wait` blocks until Apple's service replies. Typical turnaround is 1–5 minutes; the `--wait` flag also returns a non-zero exit code on failure, so a rejected notarization fails the workflow loudly. If notarization fails, fetch the log with:

```bash
xcrun notarytool log <submission-id> --apple-id ... --team-id ... --password ...
```

Stapling embeds the notarization ticket into the DMG so first-launch validation works offline. We staple only the DMG (not the inner `.app`) — when the user drags the app to `/Applications`, macOS validates the staple on the DMG itself and clears the quarantine flag for the extracted app.

## 4. GitHub Actions secrets

Add to the repo settings → Secrets and variables → Actions:

| Secret | Source |
|---|---|
| `MACOS_CERTIFICATE` | `base64 -i DeveloperIDApplication.p12 \| pbcopy` from a `.p12` exported from Keychain Access (right-click cert → Export → choose `.p12`, set a strong password). |
| `MACOS_CERTIFICATE_PASSWORD` | The password set during `.p12` export. |
| `MACOS_KEYCHAIN_PASSWORD` | Any random string — used only for the temporary CI keychain. |
| `MACOS_SIGNING_IDENTITY` | The full identity string, e.g. `Developer ID Application: Piotr Wittchen (TEAMID)`. |
| `MACOS_NOTARY_APPLE_ID` | Apple ID email used for the developer account. |
| `MACOS_NOTARY_TEAM_ID` | 10-char team ID from Apple Developer portal → Membership. |
| `MACOS_NOTARY_PASSWORD` | App-specific password (not the Apple ID password). |

The `.p12` should be exported with the **private key included** — exporting only the certificate produces a file that codesign can't use.

## 5. Verification

After the first signed + notarized release lands, verify locally:

```bash
# Download the DMG from the release page (gets the quarantine xattr)
curl -LO https://github.com/.../aictl-aarch64-apple-darwin.dmg

# Gatekeeper assessment
spctl -a -vvv -t install aictl-aarch64-apple-darwin.dmg
# Expected: "accepted, source=Notarized Developer ID"

# Mount and inspect the inner app
hdiutil attach aictl-aarch64-apple-darwin.dmg
codesign --verify --deep --strict --verbose=2 /Volumes/aictl/aictl.app
spctl -a -vvv /Volumes/aictl/aictl.app
xcrun stapler validate aictl-aarch64-apple-darwin.dmg

# Functional test — open the app from /Applications without right-click
cp -R /Volumes/aictl/aictl.app /Applications/
hdiutil detach /Volumes/aictl
open /Applications/aictl.app
# Expected: app launches with no Gatekeeper dialog
```

## 6. Cost & decision

- **$99/year** for Apple Developer Program — recurring cost the project will need to budget.
- **Notarization is per-build, not per-binary** — both `aarch64-apple-darwin` and `x86_64-apple-darwin` matrix entries in release.yml each need their own submit + staple.
- If the project decides $99/year isn't justified for personal-tier distribution, the alternative is what's already in place: ad-hoc signing + README workaround. This plan has no value until the membership exists, so the first concrete action is the human one (enrol in the program). Until then, keep the current ad-hoc signing as-is.

## Open questions

- Should we also notarize the standalone `aictl` and `aictl-server` macOS binaries (the `cargo build`-produced executables, not the `.app`)? They aren't quarantined when installed via `cargo install` from source, but pre-built binaries downloaded from Releases would benefit. Probably yes — wrap them in a minimal `.pkg` or sign-and-notarize the bare Mach-O. Out of scope for v1; revisit after the desktop path works.
- Stapler can occasionally fail with transient network errors — should we add a retry loop around `xcrun stapler staple`? Wait until we see it fail in practice.

---

## Summary of file changes

- `.github/workflows/release.yml` — replace the ad-hoc sign step with: import-cert → sign-app (hardened runtime + entitlements) → existing DMG packaging → sign-DMG → notarize → staple.
- `crates/aictl-desktop/entitlements.plist` — new file, hardened-runtime entitlements for MLX / GGUF runtime codegen.
- Repo settings (not a file) — seven new GitHub Actions secrets (§4).
- `ROADMAP.md` — already updated with a pointer to this plan; remove the bullet once the first signed + notarized release ships.
