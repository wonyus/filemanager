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
| `S3FM_UPDATE_MANIFEST_URL_STABLE` | HTTPS stable manifest URL |
| `S3FM_UPDATE_MANIFEST_URL_BETA` | HTTPS beta manifest URL |
| `S3FM_UPDATE_ARTIFACT_URL_WINDOWS_X86_64` | HTTPS URL for the signed Windows updater archive |
| `TAURI_UPDATER_PUBLIC_KEY` | Public key embedded in the generated Tauri config |
| `TAURI_SIGNING_PRIVATE_KEY` | Protected Tauri updater signing key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for the protected updater key, when applicable |
| `TAURI_UPDATER_SIGNATURE_WINDOWS_X86_64` | Signature produced by the Tauri signing step |
| `WINDOWS_CERTIFICATE_THUMBPRINT` | SHA-1 thumbprint of the Authenticode certificate installed on the runner |
| `WINDOWS_TIMESTAMP_URL` | Optional HTTPS timestamp service |

`WINDOWS_SIGNING_CERTIFICATE_PATH` and
`WINDOWS_SIGNING_CERTIFICATE_PASSWORD` may be provided together when the
runner imports a certificate as part of its protected setup. They are never
required in pull-request jobs and are never printed by the validator.

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
