export const enMemoryMessages = {
  columns: { content: "Content", kind: "Kind", status: "Status", sources: "Evidence" },
  correctionPrompt:
    "Enter the subject and corrected information. This uses the current conversation scope.",
  workStatus:
    "Queued {{queued}} · Running {{running}} · Failed {{failed}} · Held {{held}} · Deferred {{deferred}}",
  localBindingUnverified: "Registered local execution policy is unavailable or does not match",
  forgetConfirm: "Forget this source and the state derived from it? This cannot be undone.",
};
