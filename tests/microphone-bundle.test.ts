import { test, expect } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { containsSource } from "./sourceContract";
test("declares the purpose string and audio-input entitlement for signed builds", () => {
  const info = readFileSync(join(import.meta.dir, "../src-tauri/Info.plist"), "utf8");
  const entitlements = readFileSync(
    join(import.meta.dir, "../src-tauri/Entitlements.plist"),
    "utf8",
  );
  const config = JSON.parse(
    readFileSync(join(import.meta.dir, "../src-tauri/tauri.conf.json"), "utf8"),
  );
  expect(containsSource(info, "NSMicrophoneUsageDescription")).toBe(true);
  expect(containsSource(entitlements, "com.apple.security.device.audio-input")).toBe(true);
  expect(config.bundle.macOS.infoPlist).toBe("Info.plist");
  expect(config.bundle.macOS.entitlements).toBe("Entitlements.plist");
});
