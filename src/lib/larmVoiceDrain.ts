import type { LarmVoiceOwner } from "./larmVoiceOwner";

/** Listening OFF drains finalized input, but a stale UI flag cannot retain the lease forever. */
export async function drainLarmVoice(
  owner: LarmVoiceOwner,
  options: {
    cancelled: () => boolean;
    busy: () => boolean;
    release: (owner: LarmVoiceOwner, drain: boolean) => Promise<void>;
    now?: () => number;
    wait?: () => Promise<void>;
    maxWaitMs?: number;
  },
): Promise<void> {
  const now = options.now ?? Date.now;
  const wait = options.wait ?? (() => new Promise<void>((resolve) => setTimeout(resolve, 100)));
  const deadline = now() + (options.maxWaitMs ?? 60_000);
  while (!options.cancelled() && owner.phase !== "starting" && options.busy() && now() < deadline)
    await wait();
  if (!options.cancelled()) await options.release(owner, owner.phase !== "starting");
}
