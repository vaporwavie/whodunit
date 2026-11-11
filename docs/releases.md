# Releases and updates

GitHub Actions builds a universal macOS app when you push a stable `vMAJOR.MINOR.PATCH` tag. The version must match `app/Cargo.toml` and be newer than every published stable release. The workflow signs and notarizes the app, then publishes its ZIP, signed Sparkle feed, delta patches and `SHA256SUMS` as assets on that release.

This flow targets macOS 12 or newer, on Apple Silicon and Intel. Linux still uses the source installer. Neither the release nor the updater installs pi, changes your pi configuration, or updates the separately linked extension.

## One-time setup

Create a GitHub Actions environment named **release**. Restrict it to release tags and, if wanted, require approval. Configure these environment secrets:

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE_P12` | Base64-encoded Developer ID Application certificate exported with its private key |
| `APPLE_CERTIFICATE_PASSWORD` | Password for that `.p12` export |
| `APPLE_SIGNING_IDENTITY` | Full identity, such as `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | Apple ID used for notarization |
| `APPLE_TEAM_ID` | Apple Developer team ID |
| `APPLE_APP_PASSWORD` | App-specific password for notarization, not your Apple ID password |
| `SPARKLE_PRIVATE_KEY` | Contents of the private key file exported below |

Generate the Sparkle signing key once, on a trusted Mac:

```sh
bash app/scripts/fetch-sparkle.sh /tmp/whodunit-sparkle
/tmp/whodunit-sparkle/bin/generate_keys --account whodunit
/tmp/whodunit-sparkle/bin/generate_keys --account whodunit -p
/tmp/whodunit-sparkle/bin/generate_keys --account whodunit -x /tmp/whodunit-update-key
```

Put the printed public key in an environment **variable** named `SPARKLE_PUBLIC_KEY`. Put the contents of `/tmp/whodunit-update-key` in the `SPARKLE_PRIVATE_KEY` **secret**, then remove that file. Keep a backup of the key in a password manager. The private key stays out of the repository, release assets and command-line arguments.

The tooling expects Sparkle's current 32-byte private seed format. It checks that the private key matches the public key before signing a feed. Do not generate a new key for each release. Changing the embedded public key can strand existing installations; read [Sparkle's key rotation rules](https://sparkle-project.org/documentation/#rotating-signing-keys) first.

Exporting the Apple certificate as base64 on macOS:

```sh
base64 -i DeveloperID.p12 | pbcopy
```

The workflow requires Developer ID signing and notarization. It does not silently publish an ad-hoc-signed build if credentials are missing. The build uses the Rust toolchain pinned in `app/rust-toolchain.toml`; Sparkle's download version and SHA-256 are pinned in `app/scripts/fetch-sparkle.sh`.

The feed URL and repository are fixed to `vaporwavie/whodunit` in `app/scripts/release.py`. A fork must change those constants before publishing its own updater-enabled builds.

## Cut a release

1. Change `package.version` in `app/Cargo.toml`.
2. Run `cargo check` from `app/` to refresh the package version in `app/Cargo.lock`. Commit the lockfile along with the version bump.
3. Push the commit and matching tag:

```sh
git tag v0.2.0
git push origin HEAD v0.2.0
```

The `Release` workflow runs the offline Rust tests and release-tool tests, builds both architectures, signs the nested Sparkle helpers and app, and submits the bundle to Apple's notary service. It staples the ticket before making the final archive.

It downloads up to three previous stable archives and asks Sparkle to generate deltas to the new version. Every generated patch is signature-verified, applied to its source app, and checked with `codesign` before publishing. Sparkle omits patches that would not save enough download size. The first release has no patches.

The workflow uploads everything to a draft, then publishes it as latest. Until that last step, existing installations keep seeing the previous feed. A failed upload leaves a draft that a rerun can finish. Never replace assets on a published release or move its tag. Fix a bad release with a higher version.

## What users see

Download `Whodunit-<version>-macos-universal.zip` from [Releases](https://github.com/vaporwavie/whodunit/releases/latest), unzip it and move `Whodunit.app` to `~/Applications` or `/Applications` before launching. Set up pi and the extension separately as described in the [root README](../README.md#setup).

Release builds check for updates automatically through Sparkle. **Check for Updates…** in Whodunit's menu bar icon checks immediately. Sparkle shows the download and installation prompts; installation and relaunch require your consent. Automatic silent installation is disabled.

For one of the previous three releases, Sparkle can download a delta instead of the whole app. It verifies the signature and patched bundle. If a patch is missing or cannot apply, it downloads the full ZIP. Both the feed and update payloads are signed with Ed25519; archives are verified before extraction. A delta still transfers data, and savings depend on how much the build changed.

The updater replaces the app bundle, not your sessions, scores, roles or API keys. It does not call `--service-stop`. The background service retains the code it started with, so let its work finish and stop it explicitly when you want the new service build:

```sh
~/Applications/Whodunit.app/Contents/MacOS/whodunit --service-stop
```

Then reopen the app. If a release changes the private service protocol, the app reports that the old service must be stopped instead of killing agents automatically. Follow the same finish-work-first rule before installing such a release.

Source installs from `app/scripts/bundle.sh` have no Sparkle framework, update feed or update menu item. They stay on the source-build path, so the public updater cannot overwrite a development build. Existing source installations need one initial release download to opt in.

## Local checks

No Apple credentials, GitHub writes or model calls are needed:

```sh
cd app
python3 -m unittest discover -s scripts -p 'test_*.py'
cargo test --locked
cargo clippy --locked --all-targets
cargo fmt --check

# macOS-only integration test: temporary keys and ad-hoc-signed fixture apps.
bash scripts/fetch-sparkle.sh /tmp/whodunit-sparkle
SPARKLE_TEST_TOOLS=/tmp/whodunit-sparkle python3 -m unittest discover -s scripts -p 'test_*.py'
```

The integration test covers a signed first-release feed, delta generation and application, and a mismatched signing key. Before the first public update, also test the actual installation prompt and relaunch between two Developer ID-signed, notarized releases. Use genuine archived bundles: editing an installed version's plist invalidates its signature and delta checksum.

For updater errors, inspect Console.app messages for Whodunit and Sparkle. To retry an automatic check without waiting for Sparkle's schedule:

```sh
defaults delete io.github.vaporwavie.whodunit SULastCheckTime
```
