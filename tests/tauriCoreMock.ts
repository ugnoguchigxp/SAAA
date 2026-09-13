import { mock } from "bun:test";

export const invokeCalls: Array<{ command: string; args?: unknown; options?: unknown }> = [];
export const invokeImpl = {
  handler: async (command: string, _args?: unknown, _options?: unknown): Promise<unknown> => command,
};
export const channels: Array<{ onmessage: ((event: unknown) => void) | null }> = [];

export class FakeChannel<T = unknown> {
  onmessage: ((event: T) => void) | null = null;
  constructor() {
    channels.push(this as { onmessage: ((event: unknown) => void) | null });
  }
}

export function resetTauriCoreMock() {
  invokeCalls.length = 0;
  channels.length = 0;
  invokeImpl.handler = async (command) => command;
}

mock.module("@tauri-apps/api/core", () => ({
  Channel: FakeChannel,
  invoke: async (command: string, args?: unknown, options?: unknown) => {
    invokeCalls.push({ command, args, options });
    return invokeImpl.handler(command, args, options);
  },
}));
