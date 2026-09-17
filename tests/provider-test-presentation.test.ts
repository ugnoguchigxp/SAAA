import { describe, expect, test } from "bun:test";
import {
  classifyProviderTestFailure,
  credentialStorageSupport,
} from "../src/features/settings/providerTestPresentation";

describe("provider connection presentation", () => {
  test("separates authentication, timeout, unreachable, and unknown failures", () => {
    expect(classifyProviderTestFailure("HTTP 401 unauthorized")).toBe("authentication");
    expect(classifyProviderTestFailure("request timed out")).toBe("timeout");
    expect(classifyProviderTestFailure("connection refused by endpoint")).toBe("unreachable");
    expect(classifyProviderTestFailure("Provider is currently unavailable.")).toBe("unreachable");
    expect(classifyProviderTestFailure("Provider connection ended before completion.")).toBe(
      "unreachable",
    );
    expect(classifyProviderTestFailure("malformed response")).toBe("other");
  });

  test("warns before API-key operations on unsupported operating systems", () => {
    expect(credentialStorageSupport("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe(
      "supported",
    );
    expect(credentialStorageSupport("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(
      "unsupported",
    );
    expect(credentialStorageSupport("")).toBe("unknown");
  });
});
