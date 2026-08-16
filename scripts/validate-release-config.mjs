#!/usr/bin/env node

/**
 * Validate release-only configuration without ever printing secret values.
 * Normal CI may run this script with no environment and receives a clear
 * "not configured" result. A release job must pass --strict, which requires
 * updater inputs and, unless explicitly opted into certificate-free mode,
 * Authenticode inputs before it can produce artifacts.
 */

import { pathToFileURL } from "node:url";

const PLACEHOLDER_PATTERN =
  /<[^>]+>|\$\{[^}]+\}|\bTODO\b|\bCHANGE_ME\b|example\.invalid|example\.com|localhost|127\.0\.0\.1/i;
const SEMVER_PATTERN =
  /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

export function value(name) {
  const raw = process.env[name];
  return typeof raw === "string" && raw.trim() ? raw.trim() : undefined;
}

function validateHttpsUrl(errors, name, { required = false } = {}) {
  const raw = value(name);
  if (!raw) {
    if (required) errors.push(`${name} is required`);
    return undefined;
  }
  if (PLACEHOLDER_PATTERN.test(raw))
    errors.push(`${name} contains a placeholder or local URL`);
  let parsed;
  try {
    parsed = new URL(raw);
  } catch {
    errors.push(`${name} must be an absolute HTTPS URL`);
    return undefined;
  }
  if (parsed.protocol !== "https:") errors.push(`${name} must use HTTPS`);
  if (parsed.username || parsed.password)
    errors.push(`${name} must not contain credentials`);
  if (parsed.search || parsed.hash)
    errors.push(`${name} must not contain a query or fragment`);
  return raw;
}

export function validateReleaseConfig({
  strict = false,
  requireManifest = true,
  allowUnsignedWindows = false,
} = {}) {
  const errors = [];
  const warnings = [];
  const version = value("S3FM_RELEASE_VERSION");
  const channel = value("S3FM_RELEASE_CHANNEL") || "stable";
  if (version && !SEMVER_PATTERN.test(version))
    errors.push("S3FM_RELEASE_VERSION must be valid semver");
  if (strict && !version) errors.push("S3FM_RELEASE_VERSION is required");
  if (!["stable", "beta"].includes(channel))
    errors.push("S3FM_RELEASE_CHANNEL must be stable or beta");

  const stableManifestUrl = validateHttpsUrl(
    errors,
    "S3FM_UPDATE_MANIFEST_URL_STABLE",
    { required: strict },
  );
  const betaManifestUrl = validateHttpsUrl(
    errors,
    "S3FM_UPDATE_MANIFEST_URL_BETA",
    { required: strict },
  );
  if (
    stableManifestUrl &&
    betaManifestUrl &&
    stableManifestUrl === betaManifestUrl
  ) {
    errors.push("stable and beta manifest URLs must be different");
  }

  const publicKey = value("TAURI_UPDATER_PUBLIC_KEY");
  if (strict && !publicKey) errors.push("TAURI_UPDATER_PUBLIC_KEY is required");
  if (publicKey && PLACEHOLDER_PATTERN.test(publicKey))
    errors.push("TAURI_UPDATER_PUBLIC_KEY contains a placeholder");

  const signingKey = value("TAURI_SIGNING_PRIVATE_KEY");
  if (strict && !signingKey)
    errors.push(
      "TAURI_SIGNING_PRIVATE_KEY is required in the protected release job",
    );
  if (signingKey && PLACEHOLDER_PATTERN.test(signingKey))
    errors.push("TAURI_SIGNING_PRIVATE_KEY contains a placeholder");
  if (signingKey && !value("TAURI_SIGNING_PRIVATE_KEY_PASSWORD")) {
    warnings.push(
      "TAURI_SIGNING_PRIVATE_KEY_PASSWORD is empty; continue only when the protected key is intentionally unencrypted",
    );
  }

  const certificateThumbprint = value("WINDOWS_CERTIFICATE_THUMBPRINT");
  if (strict && !certificateThumbprint && !allowUnsignedWindows)
    errors.push(
      "WINDOWS_CERTIFICATE_THUMBPRINT is required for Authenticode signing",
    );
  if (strict && allowUnsignedWindows && !certificateThumbprint)
    warnings.push(
      "Certificate-free Windows mode is enabled; the installer will not have Authenticode publisher trust",
    );
  if (
    certificateThumbprint &&
    !/^[0-9A-Fa-f]{40}$/.test(certificateThumbprint)
  ) {
    errors.push(
      "WINDOWS_CERTIFICATE_THUMBPRINT must be a 40-character SHA-1 thumbprint",
    );
  }
  const certificatePath = value("WINDOWS_SIGNING_CERTIFICATE_PATH");
  const certificatePassword = value("WINDOWS_SIGNING_CERTIFICATE_PASSWORD");
  if (
    (certificatePath && !certificatePassword) ||
    (!certificatePath && certificatePassword)
  ) {
    errors.push(
      "WINDOWS_SIGNING_CERTIFICATE_PATH and WINDOWS_SIGNING_CERTIFICATE_PASSWORD must be provided together",
    );
  }
  if (certificatePath && PLACEHOLDER_PATTERN.test(certificatePath))
    errors.push("WINDOWS_SIGNING_CERTIFICATE_PATH contains a placeholder");
  const timestampUrl = validateHttpsUrl(errors, "WINDOWS_TIMESTAMP_URL");

  const artifactUrl = validateHttpsUrl(
    errors,
    "S3FM_UPDATE_ARTIFACT_URL_WINDOWS_X86_64",
    { required: strict && requireManifest },
  );
  const artifactBaseUrl = validateHttpsUrl(
    errors,
    "S3FM_UPDATE_ARTIFACT_BASE_URL",
  );
  const signature = value("TAURI_UPDATER_SIGNATURE_WINDOWS_X86_64");
  if (strict && requireManifest && !signature)
    errors.push(
      "TAURI_UPDATER_SIGNATURE_WINDOWS_X86_64 is required to generate the manifest",
    );
  if (signature && PLACEHOLDER_PATTERN.test(signature))
    errors.push(
      "TAURI_UPDATER_SIGNATURE_WINDOWS_X86_64 contains a placeholder",
    );
  const pubDate = value("S3FM_RELEASE_PUB_DATE");
  if (strict && requireManifest && !pubDate)
    errors.push("S3FM_RELEASE_PUB_DATE is required");
  if (pubDate && Number.isNaN(Date.parse(pubDate)))
    errors.push("S3FM_RELEASE_PUB_DATE must be an ISO-8601 timestamp");

  const configured = [
    stableManifestUrl,
    betaManifestUrl,
    publicKey,
    signingKey,
    certificateThumbprint,
    certificatePath,
    artifactUrl,
    artifactBaseUrl,
    signature,
    pubDate,
  ].some(Boolean);
  if (!configured && !strict)
    warnings.push(
      "No release configuration is set; this is expected outside a protected release job",
    );
  return {
    errors,
    warnings,
    configured,
    channel,
    version,
    stableManifestUrl,
    betaManifestUrl,
    publicKey,
    certificateThumbprint,
    timestampUrl,
    artifactUrl,
    signature,
    pubDate,
  };
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const strict = process.argv.includes("--strict");
  const allowUnsignedWindows = process.argv.includes(
    "--allow-unsigned-windows",
  );
  const result = validateReleaseConfig({
    strict,
    requireManifest: !process.argv.includes("--config-only"),
    allowUnsignedWindows,
  });
  for (const warning of result.warnings)
    console.warn(`release config warning: ${warning}`);
  if (result.errors.length) {
    console.error(
      `release config ${strict ? "strict " : ""}validation failed:`,
    );
    for (const error of result.errors) console.error(`- ${error}`);
    process.exitCode = 2;
  } else {
    console.log(
      `release config ${strict ? "strict " : ""}validation passed${result.configured ? " (configured)" : " (not configured)"}`,
    );
  }
}
