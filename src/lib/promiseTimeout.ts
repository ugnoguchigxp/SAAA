export async function withTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number,
  message: string,
  disposeLate?: (value: T) => void,
): Promise<T> {
  let expired = false;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  const guarded = promise.then((value) => {
    if (expired) disposeLate?.(value);
    return value;
  });
  try {
    return await Promise.race([
      guarded,
      new Promise<T>((_, reject) => {
        timeout = globalThis.setTimeout(() => {
          expired = true;
          reject(new Error(message));
        }, timeoutMs);
      }),
    ]);
  } finally {
    if (timeout !== undefined) globalThis.clearTimeout(timeout);
  }
}
