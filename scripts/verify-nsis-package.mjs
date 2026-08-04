#!/usr/bin/env node

import { promises as fs } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const required = process.argv.includes("--required");
const configPath = path.join(root, "src-tauri", "tauri.conf.json");
const config = JSON.parse(await fs.readFile(configPath, "utf8"));
const errors = [];

if (config.bundle?.active !== true) errors.push("bundle.active must be true");
const targets = config.bundle?.targets;
if (!(
  targets === "all" ||
  (Array.isArray(targets) && targets.includes("nsis"))
)) {
  errors.push("bundle.targets must include nsis");
}
if (config.bundle?.windows?.nsis?.installMode !== "currentUser") {
  errors.push("NSIS installMode must be currentUser");
}
if (config.bundle?.windows?.allowDowngrades !== false) {
  errors.push("Windows allowDowngrades must be false");
}
if (
  config.bundle?.windows?.webviewInstallMode?.type !== "downloadBootstrapper"
) {
  errors.push("Windows webviewInstallMode.type must be downloadBootstrapper");
}

const configuredPath = process.env.S3FM_INSTALLER_PATH?.trim();
const version = config.version;
const artifactPath =
  (configuredPath ? path.resolve(configuredPath) : undefined) ||
  path.join(
    root,
    "src-tauri",
    "target",
    "release",
    "bundle",
    "nsis",
    `S3 File Manager_${version}_x64-setup.exe`,
  );
let artifact;
try {
  artifact = await fs.stat(artifactPath);
} catch {
  if (required) errors.push(`NSIS artifact does not exist: ${artifactPath}`);
}
if (artifact && (!artifact.isFile() || artifact.size < 1024)) {
  errors.push(
    `NSIS artifact is unexpectedly small or not a file: ${artifactPath}`,
  );
}

if (errors.length) {
  console.error("NSIS package verification failed:");
  for (const error of errors) console.error(`- ${error}`);
  process.exit(2);
}
if (artifact)
  console.log(
    `NSIS package verified: ${artifactPath} (${artifact.size} bytes)`,
  );
else
  console.log(
    "NSIS configuration verified; artifact check skipped (run with --required on a Windows build runner)",
  );
