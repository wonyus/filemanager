#!/usr/bin/env node

/**
 * Credential-free-by-default provider smoke harness.
 *
 * Configuration is supplied only through environment variables so no endpoint
 * or credential can be accidentally committed to the repository.  Running
 * `node scripts/provider-smoke.mjs --check` validates the configuration
 * shape.  `--run` uses the locally installed AWS CLI for ListBuckets,
 * HeadBucket, and (only with `--write`) a short-lived Put/Get/Delete probe.
 */

import { execFile as execFileCallback } from "node:child_process";
import { promises as fs } from "node:fs";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";

const execFile = promisify(execFileCallback);
const PROVIDERS = new Set(["awsS3", "cloudflareR2", "minio"]);
const PLACEHOLDER_PATTERN =
  /<[^>]+>|\bTODO\b|\bCHANGE_ME\b|example\.invalid|example\.com/i;

function env(name) {
  const value = process.env[name];
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function boolEnv(name) {
  const value = env(name);
  return value === "true" || value === "1";
}

function validateConfiguration({ requireCredentials = false } = {}) {
  const errors = [];
  const provider = env("S3FM_SMOKE_PROVIDER");
  const region = env("S3FM_SMOKE_REGION");
  const endpoint = env("S3FM_SMOKE_ENDPOINT");
  const bucket = env("S3FM_SMOKE_BUCKET");
  const accessKey = env("S3FM_SMOKE_ACCESS_KEY_ID");
  const secretKey = env("S3FM_SMOKE_SECRET_ACCESS_KEY");

  if (!provider || !PROVIDERS.has(provider)) {
    errors.push(
      "S3FM_SMOKE_PROVIDER must be one of awsS3, cloudflareR2, or minio",
    );
  }
  if (!bucket || !/^[a-z0-9][a-z0-9.-]{1,253}[a-z0-9]$/.test(bucket)) {
    errors.push("S3FM_SMOKE_BUCKET must be a valid S3 bucket name");
  }
  if (!region) {
    errors.push("S3FM_SMOKE_REGION is required (use auto for Cloudflare R2)");
  }

  if (provider === "cloudflareR2" && region !== "auto") {
    errors.push("Cloudflare R2 smoke tests require S3FM_SMOKE_REGION=auto");
  }
  if (provider === "awsS3" && endpoint) {
    errors.push(
      "AWS smoke tests use the SDK-managed endpoint; do not set S3FM_SMOKE_ENDPOINT",
    );
  }
  if (provider !== "awsS3" && !endpoint) {
    errors.push("S3FM_SMOKE_ENDPOINT is required for Cloudflare R2 and MinIO");
  }
  if (endpoint) {
    let parsed;
    try {
      parsed = new URL(endpoint);
    } catch {
      errors.push("S3FM_SMOKE_ENDPOINT must be an absolute http(s) URL");
    }
    if (parsed && !["http:", "https:"].includes(parsed.protocol)) {
      errors.push("S3FM_SMOKE_ENDPOINT must use http or https");
    }
    if (
      parsed &&
      parsed.protocol === "http:" &&
      !boolEnv("S3FM_SMOKE_ALLOW_INSECURE_HTTP")
    ) {
      errors.push(
        "HTTP smoke endpoints require S3FM_SMOKE_ALLOW_INSECURE_HTTP=true",
      );
    }
    // Local/private MinIO deployments are explicitly supported by the SDD;
    // placeholder hosts are rejected for every provider.
    if (PLACEHOLDER_PATTERN.test(endpoint)) {
      errors.push("S3FM_SMOKE_ENDPOINT contains a placeholder value");
    }
    if (
      provider === "cloudflareR2" &&
      parsed &&
      !parsed.hostname.endsWith(".r2.cloudflarestorage.com")
    ) {
      errors.push(
        "Cloudflare R2 smoke endpoint must be an account endpoint (*.r2.cloudflarestorage.com)",
      );
    }
  }

  if (requireCredentials) {
    if (!accessKey)
      errors.push("S3FM_SMOKE_ACCESS_KEY_ID is required for --run");
    if (!secretKey)
      errors.push("S3FM_SMOKE_SECRET_ACCESS_KEY is required for --run");
  }
  if (accessKey && accessKey.length > 256)
    errors.push("S3FM_SMOKE_ACCESS_KEY_ID is too long");
  if (secretKey && secretKey.length > 16_384)
    errors.push("S3FM_SMOKE_SECRET_ACCESS_KEY is too long");

  return { provider, region, endpoint, bucket, accessKey, secretKey, errors };
}

function endpointArgs(config) {
  return config.endpoint ? ["--endpoint-url", config.endpoint] : [];
}

function sanitizedMessage(message, secret) {
  const text = String(message).replaceAll(/\r?\n/g, " ");
  return secret ? text.replaceAll(secret, "[REDACTED]") : text;
}

async function runAws(config, args) {
  const childEnv = {
    ...process.env,
    AWS_ACCESS_KEY_ID: config.accessKey,
    AWS_SECRET_ACCESS_KEY: config.secretKey,
    AWS_DEFAULT_REGION: config.region,
  };
  const session = env("S3FM_SMOKE_SESSION_TOKEN");
  if (session) childEnv.AWS_SESSION_TOKEN = session;
  try {
    return await execFile(
      "aws",
      ["s3api", ...args, ...endpointArgs(config), "--no-cli-pager"],
      {
        env: childEnv,
        windowsHide: true,
        maxBuffer: 1024 * 1024,
      },
    );
  } catch (error) {
    const detail = error?.stderr || error?.stdout || error?.message || error;
    throw new Error(
      `AWS CLI request failed: ${sanitizedMessage(detail, config.secretKey)}`,
    );
  }
}

async function runSmoke(config, writeProbe) {
  try {
    await runAws(config, ["list-buckets", "--output", "json"]);
    console.log(`${config.provider}: ListBuckets succeeded`);
  } catch (error) {
    // ListBuckets is commonly denied while a default bucket remains usable;
    // AC-PRO-009 requires the fallback HeadBucket path to be exercised.
    console.log(
      `${config.provider}: ListBuckets denied or unavailable; testing the configured bucket`,
    );
    if (process.env.S3FM_SMOKE_VERBOSE === "true")
      console.log(sanitizedMessage(error.message, config.secretKey));
  }

  await runAws(config, ["head-bucket", "--bucket", config.bucket]);
  console.log(`${config.provider}: HeadBucket succeeded for configured bucket`);

  if (!writeProbe) return;
  if (!boolEnv("S3FM_SMOKE_ALLOW_WRITE")) {
    throw new Error(
      "--write requires S3FM_SMOKE_ALLOW_WRITE=true; the harness never writes by default",
    );
  }

  const temporary = await fs.mkdtemp(
    path.join(os.tmpdir(), "s3fm-provider-smoke-"),
  );
  const source = path.join(temporary, "probe.txt");
  const destination = path.join(temporary, "downloaded.txt");
  const content = `s3-file-manager smoke probe ${new Date().toISOString()}\n`;
  const objectKey = `s3fm-smoke/${process.pid}-${Date.now()}/probe.txt`;
  try {
    await fs.writeFile(source, content, "utf8");
    await runAws(config, [
      "put-object",
      "--bucket",
      config.bucket,
      "--key",
      objectKey,
      "--body",
      source,
    ]);
    await runAws(config, [
      "get-object",
      "--bucket",
      config.bucket,
      "--key",
      objectKey,
      destination,
    ]);
    const downloaded = await fs.readFile(destination, "utf8");
    if (downloaded !== content)
      throw new Error("Put/Get probe content mismatch");
    await runAws(config, [
      "delete-object",
      "--bucket",
      config.bucket,
      "--key",
      objectKey,
    ]);
    console.log(`${config.provider}: Put/Get/Delete probe succeeded`);
  } finally {
    await fs.rm(temporary, { recursive: true, force: true });
  }
}

async function main() {
  const run = process.argv.includes("--run");
  const writeProbe = process.argv.includes("--write");
  const hasAnySmokeConfiguration = [
    "S3FM_SMOKE_PROVIDER",
    "S3FM_SMOKE_REGION",
    "S3FM_SMOKE_ENDPOINT",
    "S3FM_SMOKE_BUCKET",
    "S3FM_SMOKE_ACCESS_KEY_ID",
    "S3FM_SMOKE_SECRET_ACCESS_KEY",
  ].some((name) => Boolean(env(name)));
  if (!run && !hasAnySmokeConfiguration) {
    console.log(
      "Provider smoke configuration is not set; skipped (use protected environment values to run it).",
    );
    return;
  }
  const config = validateConfiguration({ requireCredentials: run });
  if (config.errors.length) {
    console.error("Provider smoke configuration is invalid:");
    for (const error of config.errors) console.error(`- ${error}`);
    process.exitCode = 2;
    return;
  }
  if (!run) {
    console.log(
      "Provider smoke configuration is valid; credentials were not required.",
    );
    return;
  }
  await runSmoke(config, writeProbe);
}

main().catch((error) => {
  console.error(
    sanitizedMessage(
      error?.message || error,
      env("S3FM_SMOKE_SECRET_ACCESS_KEY"),
    ),
  );
  process.exitCode = 1;
});
