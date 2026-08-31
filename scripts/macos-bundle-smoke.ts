import { join } from "node:path";

async function plistValue(root: string, infoPlist: string, key: string) {
  const check = Bun.spawn(
    ["/usr/bin/plutil", "-extract", key, "raw", "-o", "-", infoPlist],
    { cwd: root, stdout: "pipe", stderr: "pipe" },
  );
  const value = (await new Response(check.stdout).text()).trim();
  return { value, exitCode: await check.exited };
}

export async function verifyMacBundle(root: string): Promise<void> {
  const appBundle = join(root, "src-tauri/target/debug/bundle/macos/SAAA.app");
  const infoPlist = join(appBundle, "Contents/Info.plist");
  const tauriConfig = await Bun.file(join(root, "src-tauri/tauri.conf.json")).json() as {
    identifier: string;
    version: string;
  };
  const [microphonePurpose, bundleIdentifier, bundleVersion] = await Promise.all([
    plistValue(root, infoPlist, "NSMicrophoneUsageDescription"),
    plistValue(root, infoPlist, "CFBundleIdentifier"),
    plistValue(root, infoPlist, "CFBundleShortVersionString"),
  ]);
  if (microphonePurpose.exitCode !== 0 || microphonePurpose.value.length === 0) {
    throw new Error("Desktop smoke failed: packaged Info.plist has no microphone usage description.");
  }
  if (
    bundleIdentifier.exitCode !== 0
    || bundleIdentifier.value !== tauriConfig.identifier
    || bundleVersion.exitCode !== 0
    || bundleVersion.value !== tauriConfig.version
  ) throw new Error("Desktop smoke failed: packaged app identity does not match tauri.conf.json.");

  const signature = Bun.spawn(["/usr/bin/codesign", "-dv", appBundle], {
    cwd: root,
    stdout: "pipe",
    stderr: "pipe",
  });
  const signatureDetails = await new Response(signature.stderr).text();
  if (await signature.exited !== 0 || signatureDetails.includes("TeamIdentifier=not set")) return;
  const entitlements = Bun.spawn(["/usr/bin/codesign", "-d", "--entitlements", "-", appBundle], {
    cwd: root,
    stdout: "pipe",
    stderr: "pipe",
  });
  const entitlementXml = await new Response(entitlements.stdout).text();
  if (await entitlements.exited !== 0 || !entitlementXml.includes("com.apple.security.device.audio-input")) {
    throw new Error("Desktop smoke failed: signed app has no audio-input entitlement.");
  }
}
