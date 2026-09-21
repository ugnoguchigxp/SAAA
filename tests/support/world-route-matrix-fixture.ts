const ROUTES = [
  "openai-compatible",
  "dynamic-lan",
  "shared-larm",
  "agent-session",
  "codex",
  "reasoning-mcp",
];
const TRANSITIONS = [
  "initial",
  "tool-continuation",
  "fallback",
  "scope-switch",
  "correction",
  "forget",
  "session-resume",
];
export function validMatrixOutput(): string {
  return ROUTES.flatMap((route) =>
    TRANSITIONS.map((transition) => {
      const contract = route === "reasoning-mcp" && transition === "tool-continuation";
      const denied = transition === "scope-switch";
      const expected = contract
        ? "n/a-no-host-tool-capability"
        : denied
          ? "denied-before-model-wire"
          : "current-frame-and-matching-receipt";
      return `WORLD_MATRIX_CASE=${JSON.stringify({
        case_id: `${route}:${transition}`,
        route,
        transition,
        pass: true,
        verification_level: contract ? "offline-contract" : "offline-wire",
        omission_reason: contract ? "unsupported-capability" : denied ? "scope-changed" : null,
        expected,
        actual: expected,
        request_count:
          contract || denied
            ? 0
            : transition === "fallback" && ROUTES.indexOf(route) < 3
              ? 4
              : ["tool-continuation", "fallback", "session-resume"].includes(transition)
                ? 2
                : 1,
        frame_digest: contract || denied ? null : "a".repeat(64),
        wire_digest: contract || denied ? null : "b".repeat(64),
        source_kinds: contract || denied ? [] : ["coding", "delegation", "schedule", "situation"],
      })}`;
    }),
  ).join("\n");
}
