import { acquireVerificationLock, verificationGroupLive } from "./verification-lock";

/** Desktop build phases hold the same lock, then release it before launching the app. */
export async function startLockedBuild<T extends { child: { pid?: number } }>(
  cwd: string,
  signal: AbortSignal,
  start: (env: NodeJS.ProcessEnv, detached: boolean) => T,
) {
  const lock = await acquireVerificationLock(cwd, signal);
  let build: T;
  try {
    signal.throwIfAborted();
    build = start(lock.env, !lock.inherited);
    lock.track(build.child.pid);
  } catch (cause) {
    lock.release();
    throw cause;
  }
  return {
    build,
    release: async () => {
      try {
        if (!lock.inherited) {
          while (verificationGroupLive(build.child.pid)) await Bun.sleep(25);
        }
        lock.track();
      } finally {
        lock.release();
      }
    },
  };
}
