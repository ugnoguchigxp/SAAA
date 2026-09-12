import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { defaultRangeExtractor, useVirtualizer } from '@tanstack/react-virtual';
import type { ConversationMessage } from '../../lib/contracts';
import { CompletedMessage } from './ChatMessages';

/** Keyed measurement survives page prepend; focus pins at most one additional message. */
export function VirtualMessages({ messages, scrollRef }: { messages: ConversationMessage[]; scrollRef: RefObject<HTMLDivElement | null> }) {
  const [ready, setReady] = useState(false);
  useEffect(() => setReady(true), []);
  const [focused, setFocused] = useState<string | null>(null);
  const host = useRef<HTMLDivElement>(null);
  const heights = useRef(new Map<string, number>());
  const [margin, setMargin] = useState(0);
  const virtual = useVirtualizer({
    count: messages.length,
    getScrollElement: () => ready ? scrollRef.current : null,
    getItemKey: index => messages[index].id,
    estimateSize: index => heights.current.get(messages[index].id) ?? (messages[index].parts?.some(part => part.type === 'ui') ? 540 : 140),
    overscan: 3,
    useFlushSync: false,
    anchorTo: 'end',
    followOnAppend: false,
    scrollMargin: margin,
    rangeExtractor: range => {
      const items = defaultRangeExtractor(range); const index = messages.findIndex(message => message.id === focused);
      return index >= 0 ? [...new Set([...items, index])].sort((a,b) => a-b) : items;
    },
    measureElement: element => {
      const height = element.getBoundingClientRect().height;
      const id = element.getAttribute('data-message-id');
      if (id) { heights.current.set(id, height); if (heights.current.size > 300) heights.current.delete(heights.current.keys().next().value!); }
      return height;
    },
  });
  useLayoutEffect(() => {
    if (!host.current || !scrollRef.current) return;
    setMargin(host.current.offsetTop - scrollRef.current.offsetTop);
  }, [messages.length, scrollRef, ready]);
  useEffect(() => {
    // Virtual keeps measured sizes after eviction; bound that cache as well as our estimates.
    const retained = new Set(messages.map(message => message.id));
    for (const key of virtual.itemSizeCache.keys()) if (!retained.has(String(key))) virtual.itemSizeCache.delete(key);
  }, [messages, virtual]);
  // ResizeObserver in measureElement updates visible rows; retained offscreen sizes remain estimates.
  return <div ref={host} className="virtual-messages" style={{ height: virtual.getTotalSize(), position: 'relative', width: '100%' }}>
    {virtual.getVirtualItems().map(item => <div key={item.key} data-index={item.index} data-message-id={messages[item.index].id} ref={virtual.measureElement} style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${item.start - margin}px)` }} onFocusCapture={() => setFocused(messages[item.index].id)} onBlurCapture={event => { if (!event.currentTarget.contains(event.relatedTarget as Node)) setFocused(null); }}><CompletedMessage message={messages[item.index]} /></div>)}
  </div>;
}
