---
name: droppen-notify
description: >-
  Push a notification to the phone of the person running this Droppen
  server, for the moments they would want to be interrupted. Use it when you are
  blocked on a decision, a credential or an approval and would otherwise sit
  waiting; when you have found something they would want to stop and look at
  (data at risk, a wrong premise, production broken, a secret committed); when a
  long task changes shape or passes a milestone that changes what they should
  expect; and when work they walked away from is genuinely finished. Also use it
  when they say "notify me", "ping me", "let me know when", "tell me if",
  "I'm heading out", "text me", or "don't make me watch this". Do not use it for
  routine progress, after individual tool calls, or for anything they would
  rather have read in the transcript later.
---

# droppen-notify — interrupting someone on purpose

The person who started this session is not watching it. They are away from the
keyboard, and the transcript you are writing is not reaching them. `droppen-notify`
is the one channel that does: it puts a single line on their lock screen.

Everything below follows from that. A notification is a small, irreversible
claim on someone's attention. Sent at the right moment it saves them hours;
sent at the wrong one it teaches them to swipe the next one away unread — and
the next one is the one that mattered. You are not deciding whether something
is interesting. You are deciding whether it is worth a phone buzzing during
dinner.

## Running it

```bash
droppen-notify "Blocked: which database?" "The schema assumes Postgres but the compose file starts SQLite. Which one is real?" --level time-sensitive
```

- **First argument is the title.** It is what they read on the lock screen.
- **Second argument is the body.** Everything after it is joined into the body,
  so quoting matters less than getting the words right.
- `--level passive | active | time-sensitive` — how loudly it arrives.
  `active` is the default and is right for almost everything. Use
  `time-sensitive` only when you are blocked and the wait is real: it breaks
  through Focus modes. Use `passive` for "finished, nothing needed from you",
  which lands in the notification list without lighting the screen.
- `--group <name>` — how the phone stacks them. Defaults to this session's
  title, which is usually what you want. Give it a name when several sessions
  are working on one thing.

It exits `0` when the notification went out, and non-zero with a one-line reason
when it did not. If it says notifications are not configured, that is final for
this session: say so in the transcript and carry on. Do not retry it, and do not
look for another way to send one.

There is no reply channel. The notification is a one-way tap on the shoulder;
the answer, if there is one, comes back as a normal message in this session.

## When to send one

**1. You are blocked on them.** You need a decision, a credential, an approval,
a missing piece of context, or an answer to a genuine ambiguity, and you cannot
make honest progress without it. This is the most valuable case by a wide
margin: it is the difference between them learning in ten seconds and learning
in three hours. Send it, then write the question in the transcript and stop —
or, if there is a safe reversible path, take it and say in the notification that
you did.

> `droppen-notify "Need the staging DB password" "The migration is written and tested locally. It needs STAGING_DATABASE_URL, which is not in the env or 1Password. Everything else is ready to run." --level time-sensitive`

**2. You found something they would want to stop for.** Not "something
interesting" — something where continuing quietly is the wrong thing to do.
Data about to be lost, a credential committed to the repo, production already
broken, the task's premise turning out to be false, a change that is far larger
or far riskier than the request implied.

> `droppen-notify "Stopped: this migration drops a column" "0007_add_index.sql also drops users.legacy_id, which still has 40k non-null rows. I have not run it. Roll it back into two migrations, or is the drop intended?" --level time-sensitive`

**3. A long run changed shape.** For work measured in tens of minutes or more,
tell them when the answer to "when will this be done, and will it be what I
asked for" changes. A milestone that unblocks *them*, an estimate that has
doubled, an approach you had to abandon. One notification per real change, not
per step.

> `droppen-notify "Backfill is half done" "1.2M of 2.4M rows migrated, no errors. Running about 40 minutes behind the estimate; expect it to finish around 18:30 rather than 17:50."`

**4. The work is finished** — when it was long enough that they left. Say what
landed and what, if anything, is waiting on them. If nothing is waiting on them,
use `--level passive`.

> `droppen-notify "Auth refactor done" "All 214 tests pass, typecheck and clippy clean, pushed to feat/local-accounts. PR #128 is open and needs a review before it can merge."`

## When not to send one

- **After a tool call, a file edit, a test run, or a commit.** None of these are
  events in anyone's day.
- **To say you are starting**, or to acknowledge an instruction. They know.
- **For routine progress on work that is going fine.** "Still working" is not
  news. If nothing has changed about what they should expect, say nothing.
- **For anything you can just fix.** A failing test you are about to repair is
  not an interruption, it is the job.
- **For a short task.** If the work will be done in a couple of minutes, the
  transcript will reach them before the notification is worth anything.
- **Twice for the same thing.** If you already told them you were blocked, the
  second one adds nothing but noise.
- **To be polite.** No "just letting you know", no thanks, no summaries of what
  they can already see.
- **When you are guessing.** If you are not sure it clears the bar, it does not.

## Writing one

The title is read on a lock screen, at a glance, possibly in a queue of other
notifications. Front-load it.

- **Title**: what happened or what you need, in a few words. `Blocked: which
  database?` — not `Update on the current task`. No emoji, no "FYI", no name of
  the tool that produced it.
- **Body**: the single thing that lets them decide, and the options if there are
  options. Concrete numbers and names beat adjectives. Two sentences is usually
  enough; three is the most that will be read.
- **Never put a secret in it.** Notifications travel through a third-party push
  service and sit on a lock screen: no tokens, no keys, no passwords, no
  customer data. Say "the key in .env.production" and not the key.
- Skip repository-internal shorthand that only makes sense with the code open.

## Limits

A session may send six notifications in ten minutes; the seventh is refused
until the window moves. That ceiling is far above correct usage and exists only
to catch a loop. If you ever hit it, the right response is not to wait and retry
— it is to stop sending notifications and write in the transcript instead.
