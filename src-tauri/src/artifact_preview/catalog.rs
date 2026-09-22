use sha2::{Digest, Sha256};

use super::contracts::{APP_SCOPE, HTML_MEDIA_TYPE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lifecycle {
    Active,
    Archived,
    Deleted,
}

#[derive(Debug, Clone)]
pub(crate) struct ArtifactRevision {
    pub artifact_id: &'static str,
    pub revision_id: &'static str,
    pub title: &'static str,
    pub media_type: &'static str,
    pub scope: &'static str,
    pub lifecycle: Lifecycle,
    pub payload: &'static [u8],
}

impl ArtifactRevision {
    pub(crate) fn digest(&self) -> String {
        hex_sha256(self.payload)
    }

    pub(crate) fn size(&self) -> usize {
        self.payload.len()
    }
}

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) const INTERACTIVE_HTML: ArtifactRevision = ArtifactRevision {
    artifact_id: "artifact.fixture.interactive-html",
    revision_id: "rev.fixture.interactive-html.v1",
    title: "Interactive HTML",
    media_type: HTML_MEDIA_TYPE,
    scope: APP_SCOPE,
    lifecycle: Lifecycle::Active,
    payload: include_bytes!("fixtures/interactive.html"),
};

const ADVERSARIAL_IPC: ArtifactRevision = ArtifactRevision {
    artifact_id: "artifact.fixture.adversarial-ipc",
    revision_id: "rev.fixture.adversarial-ipc.v1",
    title: "Adversarial IPC",
    media_type: HTML_MEDIA_TYPE,
    scope: APP_SCOPE,
    lifecycle: Lifecycle::Active,
    payload: include_bytes!("fixtures/adversarial-ipc.html"),
};

const ADVERSARIAL_NETWORK: ArtifactRevision = ArtifactRevision {
    artifact_id: "artifact.fixture.adversarial-network",
    revision_id: "rev.fixture.adversarial-network.v1",
    title: "Adversarial network",
    media_type: HTML_MEDIA_TYPE,
    scope: APP_SCOPE,
    lifecycle: Lifecycle::Active,
    payload: include_bytes!("fixtures/adversarial-network.html"),
};

const ARCHIVED: ArtifactRevision = ArtifactRevision {
    artifact_id: "artifact.fixture.archived",
    revision_id: "rev.fixture.archived.v1",
    title: "Archived HTML",
    media_type: HTML_MEDIA_TYPE,
    scope: APP_SCOPE,
    lifecycle: Lifecycle::Archived,
    payload: b"<html><body>archived</body></html>",
};

const WRONG_TYPE: ArtifactRevision = ArtifactRevision {
    artifact_id: "artifact.fixture.plain",
    revision_id: "rev.fixture.plain.v1",
    title: "Plain text",
    media_type: "text/plain",
    scope: APP_SCOPE,
    lifecycle: Lifecycle::Active,
    payload: b"not html",
};

const FOREIGN_SCOPE: ArtifactRevision = ArtifactRevision {
    artifact_id: "artifact.fixture.foreign-scope",
    revision_id: "rev.fixture.foreign-scope.v1",
    title: "Foreign scope",
    media_type: HTML_MEDIA_TYPE,
    scope: "conversation:other",
    lifecycle: Lifecycle::Active,
    payload: b"<html><body>foreign</body></html>",
};

const DELETED: ArtifactRevision = ArtifactRevision {
    artifact_id: "artifact.fixture.deleted",
    revision_id: "rev.fixture.deleted.v1",
    title: "Deleted HTML",
    media_type: HTML_MEDIA_TYPE,
    scope: APP_SCOPE,
    lifecycle: Lifecycle::Deleted,
    payload: b"<html><body>deleted</body></html>",
};

const ALL: &[ArtifactRevision] = &[
    INTERACTIVE_HTML,
    ADVERSARIAL_IPC,
    ADVERSARIAL_NETWORK,
    ARCHIVED,
    WRONG_TYPE,
    FOREIGN_SCOPE,
    DELETED,
];

pub(crate) fn lookup(artifact_id: &str, revision_id: &str) -> Option<&'static ArtifactRevision> {
    ALL.iter()
        .find(|item| item.artifact_id == artifact_id && item.revision_id == revision_id)
}
