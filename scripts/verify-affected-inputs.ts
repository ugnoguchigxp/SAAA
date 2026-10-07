/** Path ownership for verification selection. Unknown paths widen the gate. */

export type Contract = "generated" | "size" | "quality" | "ipc";

export type AffectedOwner = {
  paths: string[];
  rustPackages: string[];
  typescriptTests: string[];
  contracts: Contract[];
  fallback: "none" | "normal" | "advance" | "full";
  reason: string;
};

export const MEDIA_TESTS = [
  "tests/media-http-api.test.ts",
  "tests/media-http-stream.test.ts",
  "tests/media-generation.test.tsx",
  "tests/media-recovery.test.tsx",
  "tests/feature-lab-preview.test.ts",
  "tests/feature-lab-preview.test.tsx",
];

/** Opt-in selected execution. Media and Rust domains stay on the full level until a later ticket. */
export const SELECTED_ALLOWLIST = [
  "src/features/chat/avatar/LightAvatarBackground.tsx",
  "src/features/chat/avatar/lightAvatarBackground.css",
  "tests/light-avatar-background.test.tsx",
  "tests/feature-lab-preview.test.ts",
  "tests/feature-lab-preview.test.tsx",
];

export const AVATAR_TESTS = SELECTED_ALLOWLIST.filter((path) => path.startsWith("tests/"));

const MEDIA_CONTRACTS = [
  "src/features/media/mediaApi.ts",
  "src/features/media/mediaApiModel.ts",
  "src/features/media/mediaContracts.ts",
];

export function ownersForPath(path: string): AffectedOwner {
  const normalized = path.replaceAll("\\", "/");
  if (isWide(normalized))
    return wide(normalized, "full", "shared build, lock, toolchain, or verify input");
  if (
    normalized.startsWith("services/feature-lab/") ||
    normalized.startsWith("scripts/feature-lab")
  ) {
    return wide(normalized, "full", "feature-lab host, launcher, or vite input");
  }
  if (SELECTED_ALLOWLIST.includes(normalized)) {
    return {
      paths: [normalized],
      rustPackages: [],
      typescriptTests: AVATAR_TESTS,
      contracts: [],
      fallback: "none",
      reason: "avatar drawing or its preview test; voice and inference stay out",
    };
  }
  if (MEDIA_CONTRACTS.includes(normalized)) {
    return {
      paths: [normalized],
      rustPackages: ["src-tauri", "services/feature-lab"],
      typescriptTests: MEDIA_TESTS,
      contracts: ["ipc"],
      fallback: "none",
      reason: "shared media API; desktop and HTTP callers stay in scope",
    };
  }
  if (
    normalized.startsWith("crates/saaa-media/") ||
    normalized.startsWith("src-tauri/src/media_generation/")
  ) {
    return {
      paths: [normalized],
      rustPackages: ["crates/saaa-media", "services/feature-lab", "src-tauri"],
      typescriptTests: MEDIA_TESTS,
      contracts: ["ipc"],
      fallback: "none",
      reason: "media execution and its desktop and lab callers",
    };
  }
  if (normalized.startsWith("src/features/media/") || MEDIA_TESTS.includes(normalized)) {
    return {
      paths: [normalized],
      rustPackages: [],
      typescriptTests: MEDIA_TESTS,
      contracts: [],
      fallback: "none",
      reason: "media UI and HTTP client",
    };
  }
  if (
    normalized.startsWith("crates/saaa-provider-routing/") ||
    normalized.startsWith("src-tauri/src/providers/service_registry")
  ) {
    return {
      paths: [normalized],
      rustPackages: [
        "crates/saaa-provider-routing",
        "crates/saaa-media",
        "services/feature-lab",
        "src-tauri",
      ],
      typescriptTests: MEDIA_TESTS,
      contracts: ["quality", "ipc"],
      fallback: "none",
      reason: "shared provider routing",
    };
  }
  if (normalized.startsWith("crates/larm-session/")) {
    return {
      paths: [normalized],
      rustPackages: [
        "crates/larm-session",
        "crates/saaa-media",
        "services/reasoning-mcp",
        "services/feature-lab",
        "src-tauri",
      ],
      typescriptTests: MEDIA_TESTS,
      contracts: ["ipc"],
      fallback: "none",
      reason: "larm-session and its dependents",
    };
  }
  if (normalized.endsWith(".sql") || normalized.includes("/persistence/schema")) {
    return wide(normalized, "full", "stored schema");
  }
  if (normalized.startsWith("src/") || normalized.startsWith("tests/")) {
    const test = /\.test\.tsx?$/.test(normalized) ? [normalized] : [];
    return {
      paths: [normalized],
      rustPackages: [],
      typescriptTests: test,
      contracts: ["generated", "size", "quality", "ipc"],
      fallback: "advance",
      reason: "unregistered frontend path",
    };
  }
  return wide(normalized, "advance", "unregistered path");
}

function wide(path: string, fallback: AffectedOwner["fallback"], reason: string): AffectedOwner {
  return {
    paths: [path],
    rustPackages: [],
    typescriptTests: [],
    contracts: ["generated", "size", "quality", "ipc"],
    fallback,
    reason,
  };
}

function isWide(path: string): boolean {
  return (
    path === "package.json" ||
    path === "vite.config.ts" ||
    path.startsWith("tsconfig") ||
    path === "Cargo.lock" ||
    path.endsWith("/Cargo.lock") ||
    path === "bun.lock" ||
    path === "src-tauri/build.rs" ||
    path.startsWith("scripts/verify") ||
    path.startsWith("scripts/frontend-tests") ||
    path.startsWith("scripts/module-size") ||
    path.startsWith("scripts/serial-command") ||
    path.startsWith(".cargo/") ||
    path.includes("rust-toolchain")
  );
}

export function mergeOwners(paths: string[]): AffectedOwner {
  const owners = paths.map(ownersForPath);
  const fallbackRank = { none: 0, normal: 1, advance: 2, full: 3 } as const;
  const fallback = owners.reduce(
    (widest, owner) =>
      fallbackRank[owner.fallback] > fallbackRank[widest] ? owner.fallback : widest,
    "none" as AffectedOwner["fallback"],
  );
  return {
    paths,
    rustPackages: unique(owners.flatMap((owner) => owner.rustPackages)),
    typescriptTests: unique(owners.flatMap((owner) => owner.typescriptTests)),
    contracts: unique(owners.flatMap((owner) => owner.contracts)),
    fallback,
    reason: unique(owners.map((owner) => owner.reason)).join("; "),
  };
}

/** Tests that would fail but are absent from a narrowed owner. */
export function ownershipGaps(owner: AffectedOwner, failingTests: string[]): string[] {
  if (owner.fallback !== "none") return [];
  return failingTests.filter((test) => !owner.typescriptTests.includes(test));
}

function unique<T>(values: T[]): T[] {
  return [...new Set(values)];
}
