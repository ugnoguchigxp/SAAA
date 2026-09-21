import { TableView } from "./components";

export function ModelStatusView({
  source,
  stateId,
}: {
  source: string;
  stateId: string;
}) {
  const columns =
    source === "larm.status"
      ? "provider,runtime,status,updatedAt"
      : source === "runtime.summary"
        ? "running,completed,failed,total"
        : source === "runtime.history"
          ? "time,count"
          : "provider,status,startedAt";
  return (
    <TableView
      source={source}
      columns={columns}
      stateId={stateId}
      kind="ModelStatus"
    />
  );
}
