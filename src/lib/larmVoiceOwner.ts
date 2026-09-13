export type LarmVoiceOwner = {
  id: string;
  conversationId: string;
  phase: "idle" | "starting" | "ready" | "closing" | "closed";
  ready?: Promise<void>;
  ending?: Promise<void>;
};
type Command = (name: string, args: Record<string, unknown>) => Promise<void>;

/** Retired owners remain reachable until the backend confirms release. */
export class LarmVoiceOwners {
  private owner: LarmVoiceOwner | null = null;
  private retiring = new Set<LarmVoiceOwner>();
  constructor(
    private command: Command,
    private id: () => string,
  ) {}
  current(): LarmVoiceOwner | null {
    return this.owner;
  }
  own(conversationId: string): LarmVoiceOwner {
    if (this.owner?.conversationId === conversationId) return this.owner;
    if (this.owner) this.retiring.add(this.owner);
    return (this.owner = { id: this.id(), conversationId, phase: "idle" });
  }
  end(current: LarmVoiceOwner, drain = false): Promise<void> {
    if (this.owner === current) this.owner = null;
    if (current.ending) return current.ending;
    if (current.phase === "closed") return Promise.resolve();
    const starting = current.phase === "starting";
    current.phase = "closing";
    this.retiring.add(current);
    const stop = () => this.command("end_larm_voice_session", { ownerId: current.id, drain });
    const work = async () => {
      // Cancel startup promptly, then close once more after its reply: IPC handlers
      // can run out of order, so an early end may have seen no backend owner yet.
      const early = stop().then(
        () => null,
        (error: unknown) => error,
      );
      if (starting) {
        await current.ready?.catch(() => undefined);
        await early;
        await stop();
      } else {
        const error = await early;
        if (error !== null) throw error;
      }
      current.phase = "closed";
      this.retiring.delete(current);
    };
    current.ending = work().catch((error) => {
      current.ending = undefined;
      throw error;
    });
    return current.ending;
  }
  async prepare(conversationId: string): Promise<LarmVoiceOwner | null> {
    const current = this.owner;
    if (!current) return null;
    if (current.conversationId !== conversationId) throw new Error("asr-cancelled");
    // Never start another lease while cleanup from an earlier owner is unconfirmed.
    for (const retired of this.retiring) await this.end(retired);
    if (this.owner !== current) throw new Error("asr-cancelled");
    if (!current.ready) {
      current.phase = "starting";
      current.ready = this.command("begin_larm_voice_session", {
        ownerId: current.id,
        conversationId,
      }).then(() => {
        if (current.phase === "starting") current.phase = "ready";
      });
    }
    try {
      await current.ready;
    } catch (error) {
      await this.end(current);
      throw error;
    }
    if (this.owner !== current || current.phase !== "ready") throw new Error("asr-cancelled");
    return current;
  }
  async fail(current: LarmVoiceOwner | null): Promise<void> {
    if (current) await this.end(current);
  }
}
