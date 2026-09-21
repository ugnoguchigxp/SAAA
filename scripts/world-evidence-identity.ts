import { gitEvidence } from "./world-evidence-git";

export function worldEvidenceIdentity(repoPath = process.cwd()): {
  code_revision: string;
  dirty_diff_digest: string;
  generated_at: string;
} {
  const evidence = gitEvidence(repoPath);
  return {
    code_revision: evidence.revision,
    dirty_diff_digest: evidence.digest,
    generated_at: new Date().toISOString(),
  };
}
