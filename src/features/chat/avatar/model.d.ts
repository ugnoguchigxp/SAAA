export type AvatarMotion =
  | "neutral"
  | "listening"
  | "thinking"
  | "speaking"
  | "curious"
  | "distant"
  | "downcast"
  | "greeting"
  | "agreeing"
  | "joyful"
  | "surprised"
  | "shy"
  | "sleepy";
export interface LightAvatar {
  canvas: HTMLCanvasElement;
  resize(): void;
  render(time?: number, motion?: AvatarMotion, elapsed?: number): void;
  beginMotion(): void;
  dispose(): void;
  stats(): { armCount: number; points: number; triangles: number; drawCalls: number };
}
export function createLightAvatar(host: HTMLElement): LightAvatar;
