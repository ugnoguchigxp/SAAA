import { LightAvatarBackground, useAvatarVisibility } from "./LightAvatarBackground";
import { useAvatarDecision } from "./useAvatarDecision";

export function ConversationAvatar({
  conversationId,
  active,
}: {
  conversationId: string;
  active: boolean;
}) {
  const { visible, reduced } = useAvatarVisibility(active);
  const { cue, error } = useAvatarDecision(
    conversationId,
    visible && !reduced && Boolean(window.WebGL2RenderingContext),
  );
  return (
    <>
      <LightAvatarBackground active={visible} reduced={reduced} cue={cue} />
      <div className="conversation-background-tint" aria-hidden="true" />
      {error && (
        <small className="avatar-status" title={error} role="status">
          アバターは静止表示中
        </small>
      )}
    </>
  );
}
