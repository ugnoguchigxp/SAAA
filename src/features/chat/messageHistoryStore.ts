import type { ConversationMessage } from '../../lib/contracts';
import { HISTORY_LIMIT, mergeMessageWindow } from './messageWindow';
export type MessagePage = { messages: ConversationMessage[]; hasMore: boolean; hasNewer: boolean; nextCursor: string | null; newerCursor: string | null };
type Direction = 'before' | 'after';
type Snapshot = { messages: ConversationMessage[]; hasMoreMessages: boolean; hasNewerMessages: boolean; loadingOlderMessages: boolean; loadingNewerMessages: boolean };
const empty = (): Snapshot => ({ messages: [], hasMoreMessages: false, hasNewerMessages: false, loadingOlderMessages: false, loadingNewerMessages: false });
const persisted = (message: ConversationMessage) => !message.id.startsWith('pending_') && !message.id.startsWith('streaming_');
/** Request generations belong to a conversation; refresh must not cancel pagination cleanup. */
export class MessageHistoryStore {
  private value = empty();
  private listeners = new Set<() => void>();
  private conversation: string | null = null;
  private generation = 0;
  private refresh = 0;
  private deferred?: MessagePage;
  constructor(private fetch: (conversation: string, cursor: string | null, direction: Direction) => Promise<MessagePage>) {}
  snapshot = () => this.value;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(value: Partial<Snapshot>) { this.value = { ...this.value, ...value }; this.listeners.forEach(fn => fn()); }
  setMessages = (update: ConversationMessage[] | ((current: ConversationMessage[]) => ConversationMessage[])) => {
    const messages = typeof update === 'function' ? update(this.value.messages) : update;
    this.update({ messages: messages.slice(-HISTORY_LIMIT), hasMoreMessages: this.value.hasMoreMessages || messages.length > HISTORY_LIMIT });
  };
  reset = (conversation: string | null) => { this.conversation = conversation; this.generation++; this.refresh++; this.deferred = undefined; this.update(empty()); };
  isBrowsingOlder = () => this.value.hasNewerMessages;
  private applyLatest(page: MessagePage) {
    if (this.value.hasNewerMessages) return;
    const current = this.value.messages.filter(persisted);
    const overlap = page.messages.length > 0 && current.some(old => old.id === page.messages[0].id);
    const messages = overlap ? mergeMessageWindow(current, page.messages, 'after') : page.messages;
    const hasMoreMessages = messages[0]?.id === page.messages[0]?.id ? page.hasMore : this.value.hasMoreMessages || messages[0]?.id !== current[0]?.id;
    this.update({ messages, hasMoreMessages, hasNewerMessages: false });
  }
  latest = async (conversation: string) => {
    const generation = this.generation; const ticket = ++this.refresh;
    const page = await this.fetch(conversation, null, 'before');
    if (this.conversation === conversation && generation === this.generation && ticket === this.refresh) {
      if (this.value.loadingOlderMessages || this.value.loadingNewerMessages) this.deferred = page;
      else this.applyLatest(page);
    }
    return page.messages;
  };
  load = async (direction: Direction) => {
    const conversation = this.conversation;
    if (!conversation || this.value.loadingOlderMessages || this.value.loadingNewerMessages) return;
    const current = this.value.messages.filter(persisted);
    const cursor = direction === 'before' ? current[0]?.id : current[current.length - 1]?.id;
    if (!cursor) return;
    const generation = this.generation;
    this.update(direction === 'before' ? { loadingOlderMessages: true } : { loadingNewerMessages: true });
    try {
      const page = await this.fetch(conversation, cursor, direction);
      if (generation !== this.generation) return;
      const previous = this.value.messages;
      const messages = mergeMessageWindow(previous, page.messages, direction);
      this.update(direction === 'before' ? {
        messages, hasMoreMessages: page.hasMore,
        hasNewerMessages: this.value.hasNewerMessages || messages[messages.length - 1]?.id !== previous[previous.length - 1]?.id,
      } : {
        messages, hasNewerMessages: page.hasNewer,
        hasMoreMessages: this.value.hasMoreMessages || messages[0]?.id !== previous[0]?.id,
      });
    } finally {
      if (generation === this.generation) {
        this.update({ loadingOlderMessages: false, loadingNewerMessages: false });
        const deferred = this.deferred; this.deferred = undefined;
        if (deferred) this.applyLatest(deferred);
      }
    }
  };
}
