#!/usr/bin/env node

import { promises as fs } from "node:fs";
import path from "node:path";
import { validateReleaseConfig, value } from "./validate-release-config.mjs";

const allowUnsignedWindows = process.argv.includes("--allow-unsigned-windows");
const config = validateReleaseConfig({ strict: true, allowUnsignedWindows });
if (config.errors.length) {
  console.error(
    "Cannot generate an updater manifest because release configuration is incomplete:",
  );
  for (const error of config.errors) console.error(`- ${error}`);
  process.exit(2);
}

const output =
  value("S3FM_UPDATE_MANIFEST_OUTPUT") ||
  path.join("dist", "updater", `${config.channel}-latest.json`);
const notes = value("S3FM_RELEASE_NOTES") || "S3 File Manager update";
const manifest = {
  version: config.version,
  notes,
  pub_date: new Date(config.pubDate).toISOString(),
  platforms: {
    "windows-x86_64": {
      signature: config.signature,
      url: config.artifactUrl,
    },
  },
};

await fs.mkdir(path.dirname(output), { recursive: true });
await fs
  .writeFile(output, `${JSON.stringify(manifest, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
  })
  .catch(async (error) => {
    if (error.code !== "EEXIST") throw error;
    throw new Error(
      `refusing to overwrite an existing updater manifest: ${output}`,
    );
  });
console.log(`wrote signed updater manifest for ${config.channel} to ${output}`);
