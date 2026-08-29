export type MicrophoneFailureCode =
  | "api-unavailable"
  | "permission-denied"
  | "device-not-found"
  | "device-unavailable"
  | "device-selection-invalid"
  | "startup-interrupted"
  | "unknown";

type MicrophoneMediaDevices = {
  getUserMedia?: (constraints: MediaStreamConstraints) => Promise<MediaStream>;
  enumerateDevices?: () => Promise<MediaDeviceInfo[]>;
};

export type MicrophoneEnvironment = {
  mediaDevices?: MicrophoneMediaDevices;
  secureContext?: boolean;
};

type MicrophoneCapability = "capture" | "device-list";

export class MicrophoneCaptureError extends Error {
  readonly code: MicrophoneFailureCode;
  readonly originalCause: unknown;

  constructor(code: MicrophoneFailureCode, message: string, originalCause?: unknown) {
    super(message);
    this.name = "MicrophoneCaptureError";
    this.code = code;
    this.originalCause = originalCause;
  }
}

export async function requestMicrophoneStream(
  audio: boolean | MediaTrackConstraints,
  environment: MicrophoneEnvironment = browserMicrophoneEnvironment(),
): Promise<MediaStream> {
  const mediaDevices = environment.mediaDevices;
  const getUserMedia = mediaDevices?.getUserMedia;
  if (typeof getUserMedia !== "function") throw apiUnavailableError(environment, "capture");
  try {
    return await getUserMedia.call(mediaDevices, { audio });
  } catch (cause) {
    throw classifyMicrophoneFailure(cause, environment);
  }
}

export async function enumerateAudioInputDevices(
  environment: MicrophoneEnvironment = browserMicrophoneEnvironment(),
): Promise<MediaDeviceInfo[]> {
  const mediaDevices = environment.mediaDevices;
  const enumerateDevices = mediaDevices?.enumerateDevices;
  if (typeof enumerateDevices !== "function") throw apiUnavailableError(environment, "device-list");
  try {
    const devices = await enumerateDevices.call(mediaDevices);
    return devices.filter((device) => device.kind === "audioinput");
  } catch (cause) {
    throw classifyMicrophoneFailure(cause, environment);
  }
}

export async function ensureMicrophoneAudioContextRunning(
  context: Pick<AudioContext, "state" | "resume">,
): Promise<void> {
  try {
    if (context.state !== "running" && context.state !== "closed") await context.resume();
  } catch (cause) {
    throw new MicrophoneCaptureError(
      "startup-interrupted",
      "Microphone audio processing could not start. Retry from the microphone button.",
      cause,
    );
  }
  if (context.state !== "running") {
    throw new MicrophoneCaptureError(
      "startup-interrupted",
      "Microphone audio processing did not enter the running state. Retry from the microphone button.",
    );
  }
}

export function microphoneErrorMessage(cause: unknown): string {
  return cause instanceof MicrophoneCaptureError
    ? cause.message
    : classifyMicrophoneFailure(cause, browserMicrophoneEnvironment()).message;
}

function browserMicrophoneEnvironment(): MicrophoneEnvironment {
  return {
    mediaDevices: typeof navigator === "undefined" ? undefined : navigator.mediaDevices,
    secureContext: typeof window === "undefined" ? undefined : window.isSecureContext,
  };
}

function apiUnavailableError(
  environment: MicrophoneEnvironment,
  capability: MicrophoneCapability,
  originalCause?: unknown,
): MicrophoneCaptureError {
  const message = environment.secureContext === false
    ? "Microphone capture requires a secure application context. Open SAAA through the desktop app or localhost."
    : capability === "capture"
      ? "Microphone capture is unavailable in this SAAA build. Reinstall or rebuild SAAA with microphone access enabled."
      : "Microphone device listing is unavailable in this SAAA build.";
  return new MicrophoneCaptureError("api-unavailable", message, originalCause);
}

function classifyMicrophoneFailure(
  cause: unknown,
  environment: MicrophoneEnvironment,
): MicrophoneCaptureError {
  if (cause instanceof MicrophoneCaptureError) return cause;
  const name = errorName(cause);
  if (environment.secureContext === false && (name === "NotAllowedError" || name === "SecurityError")) {
    return apiUnavailableError(environment, "capture", cause);
  }
  switch (name) {
    case "NotAllowedError":
      return new MicrophoneCaptureError(
        "permission-denied",
        "Microphone access was denied. Allow SAAA in your system privacy settings, then retry.",
        cause,
      );
    case "SecurityError":
      return new MicrophoneCaptureError(
        "api-unavailable",
        "Microphone capture is blocked by the current app or WebView security policy.",
        cause,
      );
    case "NotFoundError":
    case "DevicesNotFoundError":
      return new MicrophoneCaptureError(
        "device-not-found",
        "No microphone was found. Connect or enable a microphone, then retry.",
        cause,
      );
    case "NotReadableError":
    case "TrackStartError":
      return new MicrophoneCaptureError(
        "device-unavailable",
        "The microphone could not be opened. Close other apps using it or reconnect the device, then retry.",
        cause,
      );
    case "OverconstrainedError":
    case "ConstraintNotSatisfiedError":
      return new MicrophoneCaptureError(
        "device-selection-invalid",
        "The selected microphone is no longer available. Choose System default in Settings, then retry.",
        cause,
      );
    case "AbortError":
      return new MicrophoneCaptureError(
        "startup-interrupted",
        "Microphone startup was interrupted. Retry capture.",
        cause,
      );
    default: {
      const detail = errorMessage(cause);
      return new MicrophoneCaptureError(
        "unknown",
        detail ? `Microphone unavailable: ${detail}` : "Microphone unavailable for an unknown reason.",
        cause,
      );
    }
  }
}

function errorName(cause: unknown): string {
  return typeof cause === "object" && cause !== null && "name" in cause && typeof cause.name === "string"
    ? cause.name
    : "";
}

function errorMessage(cause: unknown): string {
  if (cause instanceof Error) return cause.message;
  return typeof cause === "string" ? cause : "";
}
