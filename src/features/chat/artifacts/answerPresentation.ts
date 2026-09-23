type CompletedAnswer = { id: string; conversationId: string; role: string; content: string };

let present: ((message: CompletedAnswer) => void) | null = null;

export function registerAnswerPresenter(handler: ((message: CompletedAnswer) => void) | null) {
  present = handler;
}

export function presentCompletedAnswer(message: CompletedAnswer) {
  if (message.role !== "assistant") return;
  present?.(message);
}
