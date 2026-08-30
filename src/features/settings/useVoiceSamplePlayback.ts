import { useEffect, useRef, useState } from "react";
import { readVoiceEnrollmentSample } from "../../lib/runtime";

export function useVoiceSamplePlayback(onError: (message: string) => void) {
  const [playingId, setPlayingId] = useState<string | null>(null);
  const playbackRef = useRef<{ audio: HTMLAudioElement; url: string } | null>(null);
  const playbackRequestRef = useRef(0);
  const disposedRef = useRef(false);
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

  useEffect(() => {
    disposedRef.current = false;
    return () => {
      disposedRef.current = true;
      playbackRequestRef.current += 1;
      stop();
    };
  }, []);

  async function play(sampleId: string) {
    const request = ++playbackRequestRef.current;
    let url: string | null = null;
    try {
      stop();
      setPlayingId(sampleId);
      const bytes = await readVoiceEnrollmentSample(sampleId);
      try {
        if (disposedRef.current || request !== playbackRequestRef.current) return;
        url = URL.createObjectURL(new Blob([bytes], { type: "audio/wav" }));
      } finally {
        new Uint8Array(bytes).fill(0);
      }
      const audio = new Audio(url);
      const playback = { audio, url };
      playbackRef.current = playback;
      audio.onended = () => finish(playback);
      audio.onerror = () => finish(playback, "サンプルを再生できませんでした。");
      await audio.play();
    } catch (cause) {
      if (playbackRef.current?.url === url) stop();
      else if (url) URL.revokeObjectURL(url);
      if (!disposedRef.current && request === playbackRequestRef.current) {
        setPlayingId(null);
        onErrorRef.current(cause instanceof Error ? cause.message : String(cause));
      }
    }
  }

  function finish(playback: { audio: HTMLAudioElement; url: string }, error?: string) {
    if (playbackRef.current !== playback) return;
    stop();
    if (disposedRef.current) return;
    setPlayingId(null);
    if (error) onErrorRef.current(error);
  }

  function stop() {
    const playback = playbackRef.current;
    playbackRef.current = null;
    if (!playback) return;
    playback.audio.onended = null;
    playback.audio.onerror = null;
    playback.audio.pause();
    URL.revokeObjectURL(playback.url);
  }

  return { playingId, play };
}
