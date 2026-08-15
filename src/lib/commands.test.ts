import { describe, expect, it } from "vitest";
import { formatCommandError, isPublicError } from "./commands";

describe("public command errors", () => {
  it("recognizes the stable error envelope", () => {
    expect(
      isPublicError({
        code: "DATABASE_ERROR",
        message: "Database unavailable",
      }),
    ).toBe(true);
  });

  it("never leaks an unknown error object to the UI", () => {
    expect(formatCommandError({ secret: "not for the frontend" })).toBe(
      "The operation could not be completed.",
    );
  });

  it("shows a safe structured reason for actionable command failures", () => {
    expect(
      formatCommandError({
        code: "PROVIDER_UNAVAILABLE",
        message: "The provider operation could not be completed.",
        details: { reason: "AccessDenied" },
      }),
    ).toBe("The provider operation could not be completed. (AccessDenied)");
  });
});
