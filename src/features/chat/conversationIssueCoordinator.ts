export class ConversationIssueCoordinator {
  private generation = 0;

  begin(): number {
    this.generation += 1;
    return this.generation;
  }

  isCurrent(generation: number): boolean {
    return generation === this.generation;
  }

  dispose(): void {
    this.generation += 1;
  }
}
