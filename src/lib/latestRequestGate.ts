export class LatestRequestGate {
  private generation = 0;
  private active = true;

  activate(): void {
    this.active = true;
    this.generation += 1;
  }

  begin(): number {
    this.generation += 1;
    return this.generation;
  }

  isCurrent(request: number): boolean {
    return this.active && request === this.generation;
  }

  dispose(): void {
    this.active = false;
    this.generation += 1;
  }
}
