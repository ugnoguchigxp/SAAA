import type { RuntimeFailureCode } from "../../lib/contracts";

export type RequiredContextFailureCode = Extract<
  RuntimeFailureCode,
  "required-context-overflow" | "context-scope-changed" | "required-context-unavailable"
>;

export type RequiredContextRecoveryAction = "narrow" | "correct";

export function requiredContextFailureCode(
  code: RuntimeFailureCode,
): RequiredContextFailureCode | null {
  switch (code) {
    case "required-context-overflow":
    case "context-scope-changed":
    case "required-context-unavailable":
      return code;
    default:
      return null;
  }
}

// These failures are intentional dispatch refusals. Replaying the exact same input cannot make
// the required context fit or make an invalid scope/source current, so it must not use retry.
export function isRetryableRequiredContextFailure(code: RuntimeFailureCode): boolean {
  return requiredContextFailureCode(code) === null;
}
