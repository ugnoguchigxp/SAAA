import { describe, expect, test } from "bun:test";
import { isDynamicLanHost, isLocalProviderHost, legacyDynamicLanHost } from "../src/lib/localProviderAddress";

describe("local provider addresses", () => {
  test("accepts loopback, private IPv4, IPv6, and .local hosts", () => {
    expect(isLocalProviderHost("127.0.0.1")).toBe(true);
    expect(isLocalProviderHost("10.0.0.8")).toBe(true);
    expect(isLocalProviderHost("172.16.1.1")).toBe(true);
    expect(isLocalProviderHost("192.168.0.1")).toBe(true);
    expect(isLocalProviderHost("169.254.1.1")).toBe(true);
    expect(isLocalProviderHost("[::1]")).toBe(true);
    expect(isLocalProviderHost("[fd00::1]")).toBe(true);
    expect(isLocalProviderHost("printer.local")).toBe(true);
    expect(isLocalProviderHost("8.8.8.8")).toBe(false);
    expect(isDynamicLanHost("localhost")).toBe(true);
    expect(isDynamicLanHost("host:9810")).toBe(false);
    expect(legacyDynamicLanHost("http://localhost:9810/")).toBe("localhost");
    expect(legacyDynamicLanHost("https://localhost:9810/")).toBeNull();
    expect(legacyDynamicLanHost("not a url")).toBeNull();
  });
});
