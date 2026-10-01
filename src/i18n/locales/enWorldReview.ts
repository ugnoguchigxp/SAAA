export const worldReview = {
  correctionPrompt:
    "Enter the subject and corrected information. This uses the current conversation scope.",
  workStatus:
    "Queued {{queued}} · Running {{running}} · Failed {{failed}} · Held {{held}} · Deferred {{deferred}}",
  localBindingUnverified:
    "Registered local execution policy is unavailable or does not match",
  forgetConfirm:
    "Forget this source and the state derived from it? This cannot be undone.",
  title: "Retrospective World registration",
  off: "Off",
  preview: "Select candidates only",
  apply: "Automatically register validated candidates",
  description:
    "Review finalized conversation with LocalLLM. Historical knowledge is held when its current validity cannot be verified.",
  inspect: "Inspect candidates and evidence",
  reasons: {
    queued: "Queued",
    selected: "Selected",
    applied: "Applied",
    "no-change": "No change",
    "already-registered": "Already registered",
    "historical-currentness-unverified": "Historical validity unverified",
    "evidence-window-incomplete": "Evidence exceeds window",
    "evidence-budget": "Evidence or conditions exceed budget",
    "proposal-input-changed": "Proposal inputs changed",
    "lease-expired": "Resuming after interruption",
  },
};
