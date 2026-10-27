import { useState } from "react";

/**
 * Keep-alive bookkeeping shared by the app, screen and terminal views: an id
 * mounts the first time it is selected and stays mounted (hidden) afterwards,
 * so its iframe, pane tree or terminal keeps its state while another route
 * shows; ids that leave `knownIds` are unmounted. `knownIds` is `null` until
 * the list has answered, because an empty answer that has not arrived yet must
 * not unmount everything on a refetch.
 *
 * `activeId` is the caller's id already checked against its own list, when
 * that list says it first (the screen and terminal views do; the app view
 * mounts a routed id even before its list loads).
 */
export function useVisited(activeId: string | null, knownIds: readonly string[] | null): string[] {
  const [visited, setVisited] = useState<string[]>([]);
  let next = visited;
  if (activeId && !next.includes(activeId)) next = [...next, activeId];
  if (knownIds) next = next.filter((id) => knownIds.includes(id));
  if (next.length !== visited.length || next.some((id, i) => id !== visited[i])) setVisited(next);
  return next;
}
