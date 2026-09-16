export const errors = {
  app: {
    conversationActive: "Processing response",
    conversationIdle: "Ready for a message",
    primaryConversationUnavailable:
      "The main conversation is unavailable. Restart SAAA and try again.",
    operationFailed: "SAAA could not complete that operation. Try again.",
  },
  chat: {
    voiceBlockedDuringMeeting:
      "Always-on listening is unavailable while a meeting is active or paused.",
    voiceSettingsUnavailable: "Voice settings are unavailable.",
    recordedAudioUnavailable: "Recorded audio is unavailable. Try again.",
    voiceQueueFull: "Voice processing is busy, so the latest utterance was not sent.",
    voicePendingLimit: "Too many voice requests are waiting. The latest utterance was not sent.",
    speechPlaybackFailed: "Speech playback could not be completed. Try again.",
    microphoneResumeFailed:
      "Always-on listening could not resume. Try the microphone button again.",
    voiceCaptureInitializationFailed:
      "Voice capture could not start. Try the microphone button again.",
    voiceAsrUnavailable:
      "The speech-recognition service is unavailable. Check the connection settings, then try the microphone button again.",
    voiceSessionConflict:
      "The previous voice session could not be closed. Restart SAAA and try again.",
    voiceTargetSpeakerModeUnavailable:
      "Target-speaker filtering is not yet available for always-on listening. Turn it off in Voice settings and try again.",
    operationFailed: "The conversation operation could not be completed. Try again.",
  },
  meeting: {
    transcriptionBackpressure:
      "Meeting transcription cannot keep up. Capture was paused; no queued audio was removed.",
    captureInactive: "Meeting capture is no longer active.",
    voiceSettingsUnavailable: "Voice settings are unavailable.",
    startFailed:
      "The meeting could not start. Check the microphone and ASR settings, then try again.",
    runtimeFailure:
      "The meeting runtime reported a failure. Check the meeting settings and try again.",
    operationFailed: "The meeting operation could not be completed. Try again.",
  },
  settings: {
    agentSessionEventStreamMissing:
      "Session creation succeeded, but no supported event stream endpoint was returned. Check the provider's SSE contract.",
    operationFailed: "The setting could not be updated. Check the values and try again.",
  },
  situation: { operationFailed: "Situation could not complete that operation. Try again." },
  voice: {
    targetSpeakerRejected:
      "The voice could not be confirmed as the enrolled speaker, so it was not transcribed.",
    asrLanguageNotAllowed: "The detected language is not allowed, so the utterance was not sent.",
    asrLanguageUnknown:
      "The spoken language could not be identified, so the utterance was not sent.",
    asrNoSpeech: "No speech was detected, so nothing was sent.",
    samplePlaybackFailed: "The voice sample could not be played.",
    microphoneStartupTimedOut: "Microphone startup timed out. Try again.",
    audioProcessorStartupTimedOut: "Audio processing could not start. Try again.",
    operationFailed: "The voice operation could not be completed. Try again.",
  },
  microphone: {
    secureContextRequired:
      "Microphone capture requires the SAAA desktop app or a secure local connection.",
    captureUnavailable: "Microphone capture is unavailable in this SAAA build.",
    deviceListUnavailable: "Microphone device listing is unavailable in this SAAA build.",
    permissionDenied:
      "Microphone access was denied. Allow SAAA in system privacy settings, then try again.",
    securityBlocked: "Microphone capture is blocked by the current app or WebView security policy.",
    deviceNotFound: "No microphone was found. Connect or enable one, then try again.",
    deviceUnavailable:
      "The microphone could not be opened. Close other apps using it or reconnect it, then try again.",
    deviceSelectionInvalid:
      "The selected microphone is unavailable. Choose System default in Settings, then try again.",
    startupInterrupted: "Microphone startup was interrupted. Try again.",
    processingCouldNotStart: "Microphone audio processing could not start. Try again.",
    processingDidNotStart: "Microphone audio processing did not start. Try again.",
  },
} as const;
