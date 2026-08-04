# S3 File Manager

Windows-first S3-compatible object storage manager built with Tauri 2, React, TypeScript, and Rust.

## Current status

The MVP implementation is in place for the Windows-first release:

- Tauri 2 application shell with a restricted default capability.
- React + TypeScript + Vite frontend with a typed command wrapper and Zustand store.
- Rust application boundary with stable public error envelopes.
- Transactional SQLite migration baseline in the platform application-data directory.
- Credential-store abstraction with a Windows Credential Manager adapter and a safe in-memory development adapter.
- Provider presets, profile lifecycle transactions, bucket/prefix explorer, metadata/preview/share links, and the Rust transfer queue are wired end to end.
- Recursive upload/download/copy/move/delete planning includes collision handling, cancellation checkpoints, and bounded provider batches.
- Redacted diagnostics export, settings persistence, and a per-user NSIS installer are included.
- CI checks frontend lint/test/build, Rust format/test/clippy, and Windows NSIS packaging.

The implementation specification is in [`docs/s3-file-manager/s3-file-manager-sdd.md`](docs/s3-file-manager/s3-file-manager-sdd.md). Session pause/range resume and durable multipart checkpoints are implemented. Release-only updater signing and provider smoke checks are wired through protected environment configuration; see [`docs/s3-file-manager/release-hardening.md`](docs/s3-file-manager/release-hardening.md) and [`docs/s3-file-manager/provider-smoke.md`](docs/s3-file-manager/provider-smoke.md).

## Development

Prerequisites for the desktop build are Node.js 22+, pnpm 9+, Rust stable with the MSVC target, and WebView2 on Windows. The current environment can run the frontend checks; Rust/Tauri checks require the Rust toolchain.

```powershell
pnpm install
pnpm dev
pnpm test
pnpm build
pnpm tauri dev
```

The application deliberately does not return permanent credentials to the frontend. Secrets cross the IPC boundary only as write-only inputs and are stored through the Rust credential-store abstraction.
