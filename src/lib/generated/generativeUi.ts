// Generated from Rust generative_ui/contracts.rs. Do not edit.
export type ContentPart = { "type": "text", text: string, } | { "type": "ui", instanceId: string, viewId: string, revision: number, summary: string, };

export type UiNode = { id: string, kind: string, args: Array<string>, span: number, children: Array<UiNode>, };

export type UiData = { capturedAt: string, rows: Array<Record<string, string | number | null>>, };

export type UiInstance = { id: string, viewId: string, revision: number, summary: string, definition: string, libraryVersion: number, mode: string, node: UiNode, state: Record<string, string | number | boolean>, snapshots: Record<string, UiData>, stateVersion: number, name: string | null, publishedRevision: number | null, };

export type SavedView = { id: string, name: string, description: string, tags: Array<string>, revision: number, };

export type UiViewRevision = { revision: number, summary: string, createdAt: string, };
