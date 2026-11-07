import { type ReactNode, useEffect, useState } from "react";
import { NOWRAP_LINK, SOURCE_URL } from "../page-kit";

/*
<<<<<<< HEAD
 * The security page's paths in the repository (`scripts/verify-web.mjs`,
 * `crates/droppen-proto`, …) as links to them on GitHub, at the commit
 * the served web app was built from. The build does not carry its commit:
 * its bytes must equal the release's web-manifest.json whichever commit
 * deployed it (docs/web-integrity.md). So the page asks the server that
 * served it, once: Droppen Hub names the commit at `/.well-known/droppen-build`
 * (docs/hub-api/http.md). Until it answers, and where nothing does (the local
 * program), the links point at `main`; so they do when the answer does not
 * say the commit is this repository's (`repo: "getdroppen/droppen"`), since a
 * commit of another repository would lead nowhere here. Never asked in a
 * static render.
||||||| parent of 4d1d34b (wip)
 * The security page's paths in the repository (`scripts/verify-web.mjs`,
 * `server-rs/crates/droppen-proto`, …) as links to them on GitHub, at the commit
 * the served web app was built from. The build does not carry its commit:
 * its bytes must equal the release's web-manifest.json whichever commit
 * deployed it (docs/web-integrity.md). So the page asks the server that
 * served it, once: Droppen Hub names the commit at `/.well-known/droppen-build`
 * (docs/hub-api/http.md). Until it answers, and where nothing does (the local
 * program), the links point at `main`; so they do when the answer does not
 * say the commit is this repository's (`repo: "getdroppen/droppen"`), since a
 * commit of another repository would lead nowhere here. Never asked in a
 * static render.
=======
 * Link source references to the commit reported by the serving Hub. The build
 * cannot embed it because release bytes must be deploy-independent. Accept only
 * this repository's hex commits; otherwise retain the `main` fallback. Do not
 * fetch during static rendering.
>>>>>>> 4d1d34b (wip)
 */

const SOURCE_REPO = "getdroppen/droppen";
const BUILD_INFO_PATH = "/.well-known/droppen-build";

/** Reject commits from other repositories or malformed build metadata. */
export function buildCommit(answer: unknown): string | null {
  if (answer === null || typeof answer !== "object") return null;
  const { commit, repo } = answer as { commit?: unknown; repo?: unknown };
  return repo === SOURCE_REPO && typeof commit === "string" && /^[0-9a-f]{7,64}$/.test(commit) ? commit : null;
}

export function repoHref(path: string, rev: string): string {
  if (path === "/") return SOURCE_URL;
  return `${SOURCE_URL}/${/\.[^/]*$/.test(path) ? "blob" : "tree"}/${rev}/${path}`;
}

let known: string | null | undefined;
let asking: Promise<string | null> | undefined;

function askBuildCommit(): Promise<string | null> {
  asking ??= fetch(BUILD_INFO_PATH, { cache: "no-store" })
    .then((r) => (r.ok ? r.json() : null))
    .then(buildCommit, () => null)
    .then((commit) => (known = commit));
  return asking;
}

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

export function RepoLink({ path, children }: { path: string; children?: ReactNode }) {
  const rev = useSourceRev();
  return (
    <a href={repoHref(path, rev)} className={NOWRAP_LINK}>
      {children ?? path}
    </a>
  );
}
