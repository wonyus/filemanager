import { afterEach, describe, expect, it } from "vitest";
import { validateReleaseConfig } from "../scripts/validate-release-config.mjs";

const originalEnv = { ...process.env };

afterEach(() => {
  for (const key of Object.keys(process.env)) {
    if (!(key in originalEnv)) delete process.env[key];
  }
  Object.assign(process.env, originalEnv);
});

function setProtectedBuildEnv() {
  Object.assign(process.env, {
    S3FM_RELEASE_VERSION: "1.2.3",
    S3FM_RELEASE_CHANNEL: "stable",
    S3FM_UPDATE_MANIFEST_URL_STABLE:
      "https://updates.example.test/stable-latest.json",
    S3FM_UPDATE_MANIFEST_URL_BETA:
      "https://updates.example.test/beta-latest.json",
    TAURI_UPDATER_PUBLIC_KEY: "protected-public-key",
    TAURI_SIGNING_PRIVATE_KEY: "protected-signing-key",
    WINDOWS_CERTIFICATE_THUMBPRINT: "0123456789abcdef0123456789abcdef01234567",
  });
}

describe("release configuration validation", () => {
  it("allows the build overlay before the archive signature exists", () => {
    setProtectedBuildEnv();
    const result = validateReleaseConfig({
      strict: true,
      requireManifest: false,
    });
    expect(result.errors).toEqual([]);
  });

  it("requires artifact, signature, and publication metadata for a manifest", () => {
    setProtectedBuildEnv();
    const result = validateReleaseConfig({ strict: true });
    expect(result.errors.join("\n")).toContain(
      "S3FM_UPDATE_ARTIFACT_URL_WINDOWS_X86_64 is required",
    );
    expect(result.errors.join("\n")).toContain(
      "TAURI_UPDATER_SIGNATURE_WINDOWS_X86_64 is required",
    );
    expect(result.errors.join("\n")).toContain(
      "S3FM_RELEASE_PUB_DATE is required",
    );
  });

  it("rejects insecure or credential-bearing update URLs", () => {
    setProtectedBuildEnv();
    process.env.S3FM_UPDATE_MANIFEST_URL_STABLE =
      "http://user:pass@localhost/manifest.json";
    const result = validateReleaseConfig({
      strict: true,
      requireManifest: false,
    });
    expect(result.errors.join("\n")).toMatch(
      /HTTPS|credentials|placeholder|local URL/i,
    );
  });

  it("validates the protected artifact base URL when present", () => {
    setProtectedBuildEnv();
    process.env.S3FM_UPDATE_ARTIFACT_BASE_URL = "http://updates.example.test";
    const result = validateReleaseConfig({
      strict: true,
      requireManifest: false,
    });
    expect(result.errors.join("\n")).toContain(
      "S3FM_UPDATE_ARTIFACT_BASE_URL must use HTTPS",
    );
  });
});
