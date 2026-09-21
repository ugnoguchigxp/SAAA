import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { RoutingProposal, routingEventReason } from "../src/features/chat/RoutingProposal";
import type { RoutingSnapshot } from "../src/lib/generated/runtimeEvent";

const snapshot: RoutingSnapshot = {
  active: {
    rootId: "root-1",
    runtimeRunId: "run-1",
    phase: "responding",
    revision: 2,
    activeSlot: "reasoning",
    cancelRequested: false,
    lastEventSeq: 4n,
  },
  queued: [],
  recentRootIds: ["root-1"],
  proposals: [
    {
      id: "proposal-1",
      rootId: "root-1",
      candidateId: "astra",
      estimatedCostMicros: null,
      expiresAtMs: 100n,
      status: "proposed",
      consumed: false,
    },
  ],
};

test("rr_26 proposal UI requires an explicit named candidate decision", () => {
  const html = renderToStaticMarkup(
    <RoutingProposal
      snapshot={snapshot}
      cancellingRootId={null}
      decidingProposalId={null}
      proposalError={null}
      onCancel={() => {}}
      onDecideProposal={() => {}}
    />,
  );
  expect(html).toContain("Premium候補 astra");
  expect(html).toContain("費用不明");
  expect(html).toContain("この候補を承認");
  expect(html).toContain("辞退");
});

test("rr_26 proposal UI does not report an IPC error as approval", () => {
  const html = renderToStaticMarkup(
    <RoutingProposal
      snapshot={snapshot}
      cancellingRootId={null}
      decidingProposalId="proposal-1"
      proposalError="候補は利用できません"
      onCancel={() => {}}
      onDecideProposal={() => {}}
    />,
  );
  expect(html).toContain('role="alert"');
  expect(html).toContain("候補は利用できません");
  expect(html).not.toContain("承認済み");
});

test("rr_28 routing history renders durable reasons without exposing arbitrary payloads", () => {
  const html = renderToStaticMarkup(
    <RoutingProposal
      snapshot={{ active: null, queued: [], recentRootIds: ["root-1"], proposals: [] }}
      events={[
        {
          rootId: "root-1",
          seq: 5n,
          kind: "answer_committed",
          dataJson: JSON.stringify({ reasonCode: "review-verified", privateDraft: "secret" }),
          createdAtMs: 100n,
        },
      ]}
      cancellingRootId={null}
      decidingProposalId={null}
      proposalError={null}
      onCancel={() => {}}
      onDecideProposal={() => {}}
    />,
  );
  expect(html).toContain("Routing 実行履歴");
  expect(html).toContain("最終回答を確定しました");
  expect(html).toContain("review-verified");
  expect(html).not.toContain("secret");
});

test("rr_28 routing history bounds and sanitizes allowlisted values", () => {
  const rendered = routingEventReason(JSON.stringify({ reason: `line\nbreak-${"x".repeat(200)}` }));
  expect(rendered).not.toContain("\n");
  expect(rendered).toEndWith("…");
  expect(rendered.length).toBeLessThanOrEqual(163);
});
