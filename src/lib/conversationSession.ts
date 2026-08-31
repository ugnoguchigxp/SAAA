export type ConversationSession = {
  runId: string | null;
  speechRunId: string | null;
};

export type InputOrigin = "text" | "voice";
export type PresentationMode = "visual" | "visual-and-spoken";
export type PendingConversationPrompt = {
  content: string;
  inputOrigin: InputOrigin;
  sourceId?: string;
  onSettled?: (delivered: boolean) => void;
};
export type SubmitPromptOptions = {
  retryInputMessageId?: string | null;
  inputOrigin?: InputOrigin;
  sourceId?: string;
  onSettled?: (delivered: boolean) => void;
};

export type ConversationSessionEvent =
  | { type: "runStarted"; runId: string }
  | { type: "runFinished"; runId: string }
  | { type: "speechStarted"; runId: string }
  | { type: "speechFinished"; runId: string };

export const initialConversationSession: ConversationSession = {
  runId: null,
  speechRunId: null,
};

export function transitionConversationSession(
  state: ConversationSession,
  event: ConversationSessionEvent,
): ConversationSession {
  switch (event.type) {
    case "runStarted":
      if (state.runId && state.runId !== event.runId) throw new Error("A conversation run is already active");
      return { ...state, runId: event.runId };
    case "runFinished":
      return state.runId === event.runId ? { ...state, runId: null } : state;
    case "speechStarted":
      if (state.speechRunId && state.speechRunId !== event.runId) throw new Error("A speech run is already active");
      return { ...state, speechRunId: event.runId };
    case "speechFinished":
      return state.speechRunId === event.runId ? { ...state, speechRunId: null } : state;
  }
}
