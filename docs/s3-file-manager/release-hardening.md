# Release hardening

Release signing is deliberately separate from normal development and pull
request builds. No private key, certificate, endpoint, or provider credential
belongs in this repository.

## Protected release inputs

The protected release job supplies the following environment values:

| Variable | Purpose |
| --- | --- |
| `S3FM_RELEASE_VERSION` | Semver version for the updater manifest |
| `S3FM_RELEASE_CHANNEL` | `stable` or `beta` |
| `S3FM_RELEASE_PUB_DATE` | ISO-8601 publication timestamp |
| `S3FM_RELEASE_NOTES` | Human-readable release notes |
| `S3FM_UPDATE_MANIFEST_URL_STABLE` | Derived GitHub Release URL for the stable manifest |
| `S3FM_UPDATE_MANIFEST_URL_BETA` | Derived GitHub Release URL for the rolling beta manifest |
| `S3FM_UPDATE_ARTIFACT_BASE_URL` | Derived GitHub Release download directory for the selected channel |
| `S3FM_UPDATE_ARTIFACT_URL_WINDOWS_X86_64` | HTTPS URL for the signed Windows updater archive |
| `TAURI_UPDATER_PUBLIC_KEY` | Public key embedded in the generated Tauri config |
| `TAURI_SIGNING_PRIVATE_KEY` | Protected Tauri updater signing key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for the protected updater key, when applicable |
| `TAURI_UPDATER_SIGNATURE_WINDOWS_X86_64` | Signature produced by the Tauri signing step |
| `WINDOWS_CERTIFICATE_THUMBPRINT` | SHA-1 thumbprint of the Authenticode certificate installed on the runner |
| `WINDOWS_TIMESTAMP_URL` | Optional HTTPS timestamp service |
| `WINDOWS_CERTIFICATE` / `WINDOWS_CERTIFICATE_PASSWORD` | Optional base64-encoded PFX and password imported by the protected workflow |

`WINDOWS_SIGNING_CERTIFICATE_PATH` and
`WINDOWS_SIGNING_CERTIFICATE_PASSWORD` may be provided together when the
runner imports a certificate as part of its protected setup. They are never
required in pull-request jobs and are never printed by the validator.

The three update URL values in the table are derived by the workflow and do
not need to be stored as secrets. Stable builds resolve through the GitHub
`latest` release endpoint; beta builds resolve through the rolling
`beta-latest` release endpoint. The workflow publishes the generated files to
those endpoints after signing.

## Commands

```text
pnpm release:validate
node scripts/validate-release-config.mjs --strict
pnpm release:tauri-config
pnpm release:manifest
```

The non-strict validator is safe in ordinary CI and only checks values that
are present. The strict mode fails closed when any signing, URL, manifest, or
Authenticode input is missing, malformed, local, or a placeholder. The two
generator scripts write ignored files under `dist/` and refuse to overwrite
an existing output. The private key remains in the runner environment.

The certificate-free path is an explicit opt-in for environments that do not
have a Windows Authenticode certificate:

```text
node scripts/validate-release-config.mjs --strict --config-only --allow-unsigned-windows
node scripts/generate-tauri-release-config.mjs --allow-unsigned-windows
node scripts/generate-update-manifest.mjs --allow-unsigned-windows
```

This mode still requires `TAURI_UPDATER_PUBLIC_KEY`,
`TAURI_SIGNING_PRIVATE_KEY`, and the updater signing password when applicable.
It omits `certificateThumbprint` from the generated Tauri overlay and builds
the NSIS installer with `--no-sign`. The updater archive and manifest remain
Tauri-signed, so update authenticity and tamper rejection are preserved. The
installer itself is not publisher-signed: Windows may show Unknown Publisher
or a SmartScreen warning on the first install.

`src-tauri/tauri.conf.json` keeps the development installer safe by default:
per-user NSIS installation, WebView2 bootstrapper handling, and downgrade
blocking. The generated release overlay additionally enables signed updater
artifacts and injects only the validated public key and HTTPS endpoints.

After a Windows build, run:

```text
pnpm verify:installer -- --required
```

This verifies the NSIS configuration and that the expected non-empty installer
artifact exists. Clean-VM installation, WebView2 offline behavior, signature
tamper rejection, and uninstall-data preservation remain release-runner tests
because they require a Windows VM and protected signing material.

The protected workflow derives the update URLs from GitHub Releases, then
validates the build overlay with
`node scripts/validate-release-config.mjs --strict --config-only`. The Tauri
build creates the updater archive and its `.sig` file; only then does the
workflow derive `S3FM_UPDATE_ARTIFACT_URL_WINDOWS_X86_64` and generate the
static channel manifest. This avoids inventing a signature before protected
signing runs. Stable releases use the `latest` GitHub Release endpoint; beta
releases use a rolling `beta-latest` release tag.

The manual workflow is `.github/workflows/release.yml`. It publishes a GitHub
Release with the installer, updater archive, signature, channel manifest, and
`checksums.txt`, and also uploads the same files as a CI artifact. Stable and
beta manifest URLs are intentionally separate. The GitHub Release permission
is protected by the `protected-release` environment.

For a certificate-free development build, use
`.github/workflows/preview-release.yml`. It publishes a prerelease with an
unsigned NSIS installer and `checksums.txt` only. It deliberately does not
enable the updater feature.

For a certificate-free release that still supports in-app updates, use
`.github/workflows/certificate-free-release.yml`. It requires only the three
protected Tauri updater secrets, publishes the signed updater archive,
signature, and channel manifest, and intentionally omits Authenticode. Users
must install that first build manually; once installed, later in-app updates
can use the signed updater. The release notes always disclose the expected
Windows publisher/SmartScreen warning.

The optional `updater` Cargo feature is enabled only by the protected release
workflows. Development/PR binaries return a typed “not configured” result from
Check updates and cannot install an update. Release binaries use the Tauri
updater plugin, verify the embedded public key and manifest signature, require
the exact `INSTALL UPDATE <version>` confirmation phrase, and check active
transfers both before download and immediately before installation.

The standard protected workflow still requires an Authenticode certificate to
provide Windows publisher trust. No secret values belong in repository files,
generated overlays, logs, or diagnostics exports.
