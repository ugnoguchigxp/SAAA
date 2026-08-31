/// <reference lib="webworker" />

import { StatefulMeetingResampler, type NormalizedMeetingSegment } from "./meetingAudioResampler";

type WorkerInput =
  | { type: "configure"; sourceRate: number; startedAtMs: number }
  | { type: "samples"; samples: Float32Array }
  | { type: "flush" };

type WorkerOutput =
  | ({ type: "segment" } & NormalizedMeetingSegment)
  | { type: "flushed" };

const scope = self as unknown as {
  onmessage: ((event: MessageEvent<WorkerInput>) => void) | null;
  postMessage: (message: WorkerOutput, transfer?: Transferable[]) => void;
};

let resampler: StatefulMeetingResampler | null = null;

scope.onmessage = (event) => {
  switch (event.data.type) {
    case "configure":
      resampler?.flush();
      resampler = new StatefulMeetingResampler(
        event.data.sourceRate,
        event.data.startedAtMs,
        (segment) => scope.postMessage({ type: "segment", ...segment }, [segment.samples.buffer]),
      );
      break;
    case "samples":
      resampler?.append(event.data.samples);
      event.data.samples.fill(0);
      break;
    case "flush":
      resampler?.flush();
      resampler = null;
      scope.postMessage({ type: "flushed" });
      break;
  }
};
