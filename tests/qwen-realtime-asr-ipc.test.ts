import { expect, test } from "bun:test";
import { qwenAsrAudioPayload } from "../src/lib/qwenAsrAudioPayload";

test("serializes nested PCM bytes as a Vec-compatible JSON array", () => {
  const payload = qwenAsrAudioPayload("session", "utterance", new Uint8Array([0, 127, 255]));
  expect(JSON.parse(JSON.stringify(payload))).toEqual({
    input: { sessionId: "session", utteranceId: "utterance", audio: [0, 127, 255] },
  });
});
