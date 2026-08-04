#!/usr/bin/env node

/**
 * Build the updater portion of a Tauri release config from protected
 * environment values. The generated file is intentionally not tracked; it
 * contains the public updater key and deployment URLs, while the private
 * signing key remains in the release runner's environment.
 */

import { promises as fs } from "node:fs";
import path from "node:path";
import { validateReleaseConfig, value } from "./validate-release-config.mjs";

const config = validateReleaseConfig({ strict: true });
if (config.errors.length) {
  console.error(
    "Cannot generate Tauri release config because release configuration is incomplete:",
  );
  for (const error of config.errors) console.error(`- ${error}`);
  process.exit(2);
}

const output =
  value("S3FM_TAURI_RELEASE_CONFIG_OUTPUT") ||
  path.join("dist", "tauri.release.conf.json");
const generated = {
  $schema: "https://schema.tauri.app/config/2",
  bundle: {
    createUpdaterArtifacts: "v1Compatible",
    windows: {
      digestAlgorithm: "sha256",
      certificateThumbprint: config.certificateThumbprint,
      webviewInstallMode: {
        type: "downloadBootstrapper",
        silent: true,
      },
      allowDowngrades: false,
      nsis: {
        installMode: "currentUser",
        displayLanguageSelector: true,
      },
    },
  },
  plugins: {
    updater: {
      pubkey: config.publicKey,
      endpoints: [config.stableManifestUrl, config.betaManifestUrl],
    },
  },
};
if (config.timestampUrl)
  generated.bundle.windows.timestampUrl = config.timestampUrl;

await fs.mkdir(path.dirname(output), { recursive: true });
await fs
  .writeFile(output, `${JSON.stringify(generated, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
  })
  .catch(async (error) => {
    if (error.code !== "EEXIST") throw error;
    throw new Error(
      `refusing to overwrite an existing Tauri release config: ${output}`,
    );
  });
console.log(`wrote Tauri release config to ${output}`);
