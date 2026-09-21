import { stewardApi } from "./stewardApi";

export function DelegationConfirmation({
  conversationId,
  proposalId,
  digest,
  revision,
  onDone,
}: {
  conversationId: string;
  proposalId: string;
  digest: string;
  revision: number;
  onDone: () => void;
}) {
  return (
    <div>
      <p>この委任内容で一度だけ許可します。</p>
      <button
        type="button"
        onClick={() =>
          void stewardApi
            .confirmProposal(conversationId, proposalId, revision, digest, true)
            .then(onDone)
        }
      >
        登録して開始
      </button>
      <button
        type="button"
        onClick={() =>
          void stewardApi
            .confirmProposal(conversationId, proposalId, revision, digest, false)
            .then(onDone)
        }
      >
        委任だけ登録
      </button>
    </div>
  );
}
