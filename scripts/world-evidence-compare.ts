type Identity = { code_revision: string; dirty_diff_digest: string };

export function sameWorldEvidenceIdentity(left: Identity, right: Identity): boolean {
  return (
    left.code_revision === right.code_revision && left.dirty_diff_digest === right.dirty_diff_digest
  );
}
