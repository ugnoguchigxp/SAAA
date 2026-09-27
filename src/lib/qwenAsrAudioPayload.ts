/** Tauri serializes nested IPC values as JSON, so PCM bytes must be an array. */
export function qwenAsrAudioPayload(sessionId: string, utteranceId: string, audio: Uint8Array) {
  return { input: { sessionId, utteranceId, audio: Array.from(audio) } };
}
