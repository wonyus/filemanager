# S3 File Manager

Windows-first S3-compatible object storage manager built with Tauri 2, React, TypeScript, and Rust.

## Current status

Phase 0 foundation is in place:

- Tauri 2 application shell with a restricted default capability.
- React + TypeScript + Vite frontend with a typed command wrapper and Zustand store.
- Rust application boundary with stable public error envelopes.
- Transactional SQLite migration baseline in the platform application-data directory.
- Credential-store abstraction with a Windows Credential Manager adapter and a safe in-memory development adapter.
- CI checks for frontend lint/test/build and Rust format/test/clippy.

The implementation specification is in [`docs/s3-file-manager/s3-file-manager-sdd.md`](docs/s3-file-manager/s3-file-manager-sdd.md). Profile CRUD and S3 provider integration are the next Phase 1 increment.

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
