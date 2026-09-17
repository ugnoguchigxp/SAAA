export type ProviderTestFailureKind = "authentication" | "timeout" | "unreachable" | "other";

export function classifyProviderTestFailure(message: string): ProviderTestFailureKind {
  if (/\b(401|403)\b|unauthori[sz]ed|forbidden|api key|credential|authentication/i.test(message))
    return "authentication";
  if (/timed?\s*out|timeout|deadline/i.test(message)) return "timeout";
  if (
    /connection refused|connection ended|could not connect|failed to connect|dns|name or service not known|network|unreachable|unavailable/i.test(
      message,
    )
  )
    return "unreachable";
  return "other";
}

export function credentialStorageSupport(
  userAgent: string,
): "supported" | "unsupported" | "unknown" {
  if (/Macintosh|Mac OS X/i.test(userAgent)) return "supported";
  if (/Windows|Linux|Android|iPhone|iPad/i.test(userAgent)) return "unsupported";
  return "unknown";
}
