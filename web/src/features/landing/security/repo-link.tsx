import { type ReactNode, useEffect, useState } from "react";
import { SOURCE_URL } from "../page-kit";

/*
 * The security page's paths in the repository (`scripts/verify-web.mjs`,
 * `server-rs/crates/rh-proto`, …) as links to them on GitHub, at the commit
 * the served web app was built from. The build does not carry its commit:
 * its bytes must equal the release's web-manifest.json whichever commit
 * deployed it (docs/web-integrity.md). So the page asks the server that
 * served it, once: Droppen Hub names the commit at `/.well-known/droppen-build`
 * (docs/hub-api/http.md). Until it answers, and where nothing does (the local
 * program), the links point at `main`. Never asked in a static render.
 */

/** Where the server says which build it serves. */
export const BUILD_INFO_PATH = "/.well-known/droppen-build";

/** The commit a `/.well-known/droppen-build` answer names, when it is one (7 to 64 hex digits); else null. */
export function buildCommit(answer: unknown): string | null {
  const commit = answer !== null && typeof answer === "object" ? (answer as { commit?: unknown }).commit : undefined;
  return typeof commit === "string" && /^[0-9a-f]{7,64}$/.test(commit) ? commit : null;
}

/** A path in the repository on GitHub at `rev`: `blob/` for a file (its last part has a dot), `tree/` for a directory; "/" is the repository itself. */
export function repoHref(path: string, rev: string): string {
  if (path === "/") return SOURCE_URL;
  return `${SOURCE_URL}/${/\.[^/]*$/.test(path) ? "blob" : "tree"}/${rev}/${path}`;
}

let known: string | null | undefined;
let asking: Promise<string | null> | undefined;

/** The served build's commit, asked once per page load (null: the server did not say). */
function askBuildCommit(): Promise<string | null> {
  asking ??= fetch(BUILD_INFO_PATH, { cache: "no-store" })
    .then((r) => (r.ok ? r.json() : null))
    .then(buildCommit, () => null)
    .then((commit) => (known = commit));
  return asking;
}

/** The revision the links point at: the served build's commit once known, `main` until then. */
function useSourceRev(): string {
  const [rev, setRev] = useState(() => known ?? "main");
  useEffect(() => {
    let live = true;
    void askBuildCommit().then((commit) => {
      if (live && commit) setRev(commit);
    });
    return () => {
      live = false;
    };
  }, []);
  return rev;
}

/** A path in the repository, linked on GitHub; its text the path as written, copyable as before. */
export function RepoLink({ path, children }: { path: string; children?: ReactNode }) {
  const rev = useSourceRev();
  return (
    <a href={repoHref(path, rev)} className="text-fg underline underline-offset-4 hover:no-underline">
      {children ?? path}
    </a>
  );
}
