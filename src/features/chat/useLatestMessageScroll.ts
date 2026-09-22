import { useCallback, useLayoutEffect, useRef, useState } from "react";

const LATEST_THRESHOLD_PX = 24;

/** Keep the entire conversation tail visible, including asynchronously measured content. */
export function useLatestMessageScroll(
  conversationId: string | undefined,
  hasNewerMessages: boolean,
) {
  const messageAreaRef = useRef<HTMLDivElement>(null);
  const messageContentRef = useRef<HTMLDivElement>(null);
  const followLatestRef = useRef(true);
  const [showLatestButton, setShowLatestButton] = useState(false);
  const lastPosition = useRef({ top: 0, height: 0, viewport: 0 });

  const scrollToLatest = useCallback(() => {
    const area = messageAreaRef.current;
    if (!area || !followLatestRef.current || hasNewerMessages) return;
    area.scrollTop = area.scrollHeight;
    lastPosition.current = {
      top: area.scrollTop,
      height: area.scrollHeight,
      viewport: area.clientHeight,
    };
    setShowLatestButton(false);
  }, [hasNewerMessages]);

  useLayoutEffect(() => {
    followLatestRef.current = true;
    setShowLatestButton(false);
  }, [conversationId]);

  // React commits and later measurements both change the available scroll range.
  useLayoutEffect(scrollToLatest);
  useLayoutEffect(() => {
    const area = messageAreaRef.current;
    const content = messageContentRef.current;
    if (!area || !content) return;
    const observer = new ResizeObserver(scrollToLatest);
    observer.observe(area);
    observer.observe(content);
    return () => observer.disconnect();
  }, [scrollToLatest]);

  function updateFollowLatest() {
    const area = messageAreaRef.current;
    if (!area) return;
    const previous = lastPosition.current;
    const atLatest =
      area.scrollHeight - area.scrollTop - area.clientHeight < LATEST_THRESHOLD_PX &&
      !hasNewerMessages;
    // Appending or measuring content must not be mistaken for the user scrolling up.
    const movedUp = area.scrollTop < previous.top;
    const resized =
      area.scrollHeight !== previous.height || area.clientHeight !== previous.viewport;
    if (atLatest) followLatestRef.current = true;
    else if (hasNewerMessages || (movedUp && !resized)) followLatestRef.current = false;
    lastPosition.current = {
      top: area.scrollTop,
      height: area.scrollHeight,
      viewport: area.clientHeight,
    };
    setShowLatestButton(!followLatestRef.current);
    scrollToLatest();
  }

  return {
    messageAreaRef,
    messageContentRef,
    followLatestRef,
    showLatestButton,
    setShowLatestButton,
    scrollToLatest,
    updateFollowLatest,
  };
}
