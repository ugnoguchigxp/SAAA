// Generated from artifact_preview/contracts.rs. Do not edit.
export type PrepareArtifactPreviewInput = { artifactId: string, revisionId: string, };

export type ReleaseArtifactPreviewInput = { previewToken: string, };

export type MountArtifactPreviewInput = { previewToken: string, x: number, y: number, width: number, height: number, };

export type ArtifactPreviewPolicy = { network: string, navigation: string, popup: string, download: string, tauriIpc: string, };

export type ArtifactPreviewDescriptor = { artifactId: string, revisionId: string, title: string, mediaType: string, digest: string, previewToken: string, expiresAt: string, webviewLabel: string, policy: ArtifactPreviewPolicy, };
