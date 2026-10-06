// Eight-second acting phrases: anticipation, action, hold, and recovery.
// Head angles are radians. No whole-body stretching is used.
export const motionNames = {
  listening: "聞く",
  thinking: "考える",
  speaking: "話す",
  curious: "首をかしげる",
  distant: "遠くを見る",
  downcast: "うなだれる",
  greeting: "挨拶する",
  agreeing: "うなずく",
  joyful: "うれしい",
  surprised: "驚く",
  shy: "照れる",
  sleepy: "眠そう",
};
export const motionModes = Object.keys(motionNames);
export const neutral = {
  hx: 0,
  hy: 0,
  hz: 0,
  bx: 0,
  by: 0,
  bz: 0,
  lift: 0,
  forward: 0,
  left: 0,
  right: 0,
  leftInner: 0,
  rightInner: 0,
  gx: 0,
  gy: 0,
  energy: 0.05,
  open: 1,
};
const key = (t, values = {}) => [t, { ...neutral, ...values }];
const phrases = {
  listening: [
    key(0),
    key(1, { hy: -0.1, hz: 0.08, forward: 0.07, leftInner: -0.035, rightInner: 0.03 }),
    key(2.5, { hx: 0.13, hy: -0.1, hz: 0.08, forward: 0.07, leftInner: -0.035, rightInner: 0.03 }),
    key(3.2, {
      hx: -0.025,
      hy: -0.1,
      hz: 0.08,
      forward: 0.07,
      leftInner: -0.035,
      rightInner: 0.03,
    }),
    key(4.8, { hy: -0.1, hz: 0.08, forward: 0.07, leftInner: -0.035, rightInner: 0.03 }),
    key(6.2, { hx: 0.09, hz: 0.04 }),
    key(8),
  ],
  thinking: [
    key(0),
    key(1.3, {
      hx: -0.16,
      hy: 0.23,
      hz: -0.14,
      gx: 0.03,
      gy: 0.015,
      right: -0.14,
      rightInner: 0.24,
      leftInner: -0.04,
    }),
    key(3.5, {
      hx: -0.16,
      hy: 0.23,
      hz: -0.14,
      gx: 0.03,
      gy: 0.015,
      right: -0.14,
      rightInner: 0.24,
      leftInner: -0.04,
    }),
    key(4.6, { hx: -0.08, hy: -0.18, hz: 0.09, gx: -0.025 }),
    key(6.4, { hx: -0.08, hy: -0.18, hz: 0.09, gx: -0.025 }),
    key(8),
  ],
  speaking: [
    key(0),
    key(0.8, { hx: -0.045, hy: -0.11, left: 0.18, rightInner: 0.14, energy: 0.18 }),
    key(1.5, { hx: 0.09, hy: -0.08, left: 0.09, energy: 0.23 }),
    key(2.2, { hy: 0.12, right: -0.2, leftInner: -0.16, energy: 0.12 }),
    key(3.2, { hx: 0.06, hy: 0.12, right: -0.12, energy: 0.2 }),
    key(4.1),
    key(5.1, {
      hx: -0.07,
      hz: 0.07,
      left: 0.13,
      right: -0.09,
      leftInner: -0.1,
      rightInner: 0.08,
      energy: 0.22,
    }),
    key(6, { hx: 0.09, hz: 0.04, left: 0.07, energy: 0.15 }),
    key(7),
    key(8),
  ],
  curious: [
    key(0),
    key(0.9, { hx: -0.05, hz: -0.28, hy: 0.07, gx: 0.018, forward: 0.05 }),
    key(3.4, { hx: -0.05, hz: -0.28, hy: 0.07, gx: 0.018, forward: 0.05 }),
    key(4.3, { hx: 0.015, hz: 0.19, hy: -0.05, gx: -0.018 }),
    key(6.2, { hx: 0.015, hz: 0.19, hy: -0.05, gx: -0.018 }),
    key(8),
  ],
  distant: [
    key(0),
    key(1.6, { hx: -0.2, hy: 0.45, by: 0.1, gx: 0.035, gy: 0.02 }),
    key(5.7, { hx: -0.2, hy: 0.45, by: 0.1, gx: 0.035, gy: 0.02 }),
    key(8),
  ],
  downcast: [
    key(0),
    key(1.9, {
      hx: 0.42,
      hz: 0.055,
      bx: 0.06,
      lift: -0.11,
      gy: -0.028,
      left: 0.16,
      right: -0.16,
      leftInner: 0.11,
      rightInner: -0.09,
      energy: 0.02,
      open: 0.75,
    }),
    key(6, {
      hx: 0.42,
      hz: 0.055,
      bx: 0.06,
      lift: -0.11,
      gy: -0.028,
      left: 0.16,
      right: -0.16,
      leftInner: 0.11,
      rightInner: -0.09,
      energy: 0.02,
      open: 0.75,
    }),
    key(8),
  ],
  greeting: [
    key(0),
    key(0.9, { hy: -0.18, hz: 0.06, left: -0.29, rightInner: 0.11 }),
    key(1.6, { hy: -0.18, hz: 0.06, left: -0.13, rightInner: 0.11 }),
    key(2.2, { hy: -0.15, hz: 0.04, left: -0.29, rightInner: 0.11 }),
    key(2.9, { hy: -0.12, left: -0.13, rightInner: 0.11 }),
    key(3.5, { hy: -0.1, left: -0.26, rightInner: 0.07 }),
    key(4.3, { hx: 0.12, left: -0.1 }),
    key(5.1),
    key(8),
  ],
  agreeing: [
    key(0),
    key(0.7, { hx: -0.05 }),
    key(1.3, { hx: 0.23, forward: 0.045 }),
    key(1.95, { hx: -0.045 }),
    key(2.6, { hx: 0.19, forward: 0.035 }),
    key(3.3),
    key(6),
    key(8),
  ],
  joyful: [
    key(0),
    key(1.1, {
      hx: -0.1,
      hz: 0.12,
      lift: 0.1,
      left: -0.16,
      right: 0.16,
      leftInner: -0.07,
      rightInner: 0.1,
      open: 0.62,
      energy: 0.16,
    }),
    key(2, { hx: -0.08, hz: -0.09, lift: 0.045, left: -0.13, right: 0.13, open: 0.7 }),
    key(3, { hx: -0.09, hz: 0.07, lift: 0.075, open: 0.68 }),
    key(5, { hx: -0.07, hz: 0.07, lift: 0.05, open: 0.75 }),
    key(8),
  ],
  surprised: [
    key(0),
    key(0.55),
    key(0.9, {
      hx: -0.24,
      forward: -0.12,
      left: -0.19,
      right: 0.19,
      leftInner: -0.14,
      rightInner: 0.12,
      open: 1.22,
      energy: 0.18,
    }),
    key(2.6, {
      hx: -0.24,
      forward: -0.12,
      left: -0.19,
      right: 0.19,
      leftInner: -0.14,
      rightInner: 0.12,
      open: 1.22,
      energy: 0.18,
    }),
    key(4, { hx: -0.06, open: 1.08 }),
    key(6),
    key(8),
  ],
  shy: [
    key(0),
    key(1.3, {
      hx: 0.15,
      hy: -0.25,
      hz: 0.16,
      gx: -0.02,
      gy: -0.018,
      left: -0.08,
      right: 0.08,
      leftInner: -0.12,
      rightInner: 0.14,
      open: 0.83,
    }),
    key(4.3, {
      hx: 0.15,
      hy: -0.25,
      hz: 0.16,
      gx: -0.02,
      gy: -0.018,
      left: -0.08,
      right: 0.08,
      leftInner: -0.12,
      rightInner: 0.14,
      open: 0.83,
    }),
    key(5.2, { hx: 0.08, hy: 0.09, hz: 0.09, gx: 0.012 }),
    key(6.5, { hx: 0.08, hy: 0.09, hz: 0.09, gx: 0.012 }),
    key(8),
  ],
  sleepy: [
    key(0, { open: 0.8 }),
    key(2, { hx: 0.24, hz: 0.1, lift: -0.045, open: 0.38 }),
    key(3.7, { hx: 0.32, hz: 0.13, lift: -0.06, open: 0.12 }),
    key(4.5, { hx: -0.045, open: 0.92 }),
    key(5.7, { hx: 0.08, hz: 0.05, open: 0.7 }),
    key(8, { open: 0.8 }),
  ],
};
const ease = (t) => t * t * (3 - 2 * t);
export function blendPose(a, b, t) {
  const p = {};
  t = ease(Math.max(0, Math.min(1, t)));
  for (const k of Object.keys(neutral)) p[k] = a[k] + (b[k] - a[k]) * t;
  return p;
}
function blinkPulse(t, at, duration = 0.3) {
  const d = t - at;
  if (d < 0 || d > duration) return 0;
  const peak = duration * 0.38;
  return d < peak ? ease(d / peak) : 1 - ease((d - peak) / (duration - peak));
}
export function sampleMotion(mode, time) {
  if (mode === "neutral") return { ...neutral };
  const frames = phrases[mode] || phrases.listening,
    t = Math.max(0, Math.min(time, 8));
  let i = 0;
  while (i < frames.length - 2 && t > frames[i + 1][0]) i++;
  const [a, pa] = frames[i],
    [b, pb] = frames[i + 1];
  const p = blendPose(pa, pb, (t - a) / (b - a));
  const blinks =
    mode === "sleepy"
      ? [
          [1.35, 0.85],
          [3.1, 1.05],
          [6.2, 0.7],
        ]
      : mode === "surprised"
        ? [
            [0.2, 0.27],
            [3.2, 0.32],
            [6.4, 0.27],
          ]
        : mode === "shy"
          ? [
              [1, 0.32],
              [1.5, 0.3],
              [5.8, 0.3],
            ]
          : [
              [2.9, 0.28],
              [6.6, 0.32],
            ];
  p.open *= 1 - Math.max(...blinks.map(([at, d]) => blinkPulse(t, at, d))) * 0.97;
  return p;
}
