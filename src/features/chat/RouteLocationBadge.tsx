import { useEffect, useState } from "react";
import { getServiceRegistry, type RegistryView } from "../../lib/serviceRegistry";

const POLL_MS = 20_000;

/** Tells the user which side answers while LARM (at home) cannot be reached. No approval needed. */
export function routeLocationNotice(view: RegistryView | null): string | null {
  if (view?.larmReachability !== "unreachable") return null;
  const binding = view.snapshot.bindings.find((item) => item.purpose === "conversation.respond");
  const primary = view.snapshot.resources.find((r) => r.resourceId === binding?.primaryResourceId);
  const usesLarm =
    view.snapshot.connections.find((c) => c.connectionId === primary?.connectionId)?.adapterKind ===
    "larm";
  if (!binding?.enabled || !usesLarm) return null;
  // Only a fallback that would really be used counts: enabled, and cloud sending allowed.
  const usable = binding.fallbackResourceIds.some((id) => {
    const resource = view.snapshot.resources.find((r) => r.resourceId === id);
    const connection = view.snapshot.connections.find(
      (c) => c.connectionId === resource?.connectionId,
    );
    return (
      !!resource?.enabled &&
      !!connection?.enabled &&
      (connection.location !== "cloud" || binding.cloudAllowed !== false)
    );
  });
  return usable
    ? "LARMに接続できないため、代替先で応答します"
    : "LARMに接続できません（利用できる代替先がありません）";
}

export function RouteLocationBadge() {
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const refresh = async () => {
      try {
        const view = await getServiceRegistry();
        if (alive) setNotice(routeLocationNotice(view));
      } catch {
        if (alive) setNotice(null);
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);
  if (!notice) return null;
  return (
    <span className="route-location-badge" role="status">
      {notice}
    </span>
  );
}
