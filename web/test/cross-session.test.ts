// Two texts that arrive on the user side without the user having written them.
import assert from "node:assert/strict";
import test from "node:test";
import { crossSessionOf, droppenNoticeOf } from "../src/features/sessions/cross-session.js";

const ENVELOPE =
  '<cross-session-message from="uds:/tmp/droppen-ff.sock" from-name="droppen-ff" from-mode="prompting">';

test("a cross-session envelope yields the sender and the words", () => {
  const cross = crossSessionOf(`${ENVELOPE}\n\nThe cursor argument is agreed.\n</cross-session-message>`);
  assert.deepEqual(cross, { from: "droppen-ff", body: "The cursor argument is agreed." });
});

test("the attributes may come in any order, and the name may be missing", () => {
  assert.equal(crossSessionOf('<cross-session-message from-name="a" from="x">\nhi')?.from, "a");
  const anonymous = crossSessionOf('<cross-session-message from="x">\nhi');
  assert.deepEqual(anonymous, { from: null, body: "hi" });
});

test("XML entities in from-name are unescaped once without double-decoding", () => {
  const escaped =
    '<cross-session-message from="acp/s1" from-name="agy: fix &quot;quotes&quot; &amp; &lt;tags&gt; &apos;s&apos;" hop="1">\n\nhello\n</cross-session-message>';
  assert.deepEqual(crossSessionOf(escaped), {
    from: "agy: fix \"quotes\" & <tags> 's'",
    body: "hello",
  });

  const doubleEscaped =
    '<cross-session-message from="acp/s1" from-name="literal &amp;lt;entity&amp;gt;" hop="1">\n\nbody\n</cross-session-message>';
  assert.deepEqual(crossSessionOf(doubleEscaped), {
    from: "literal &lt;entity&gt;",
    body: "body",
  });
});

test("an ordinary message is not a cross-session one", () => {
  assert.equal(crossSessionOf("Use the rows from the last answer."), null);
  // A tag that is merely mentioned in the text stays part of the text.
  assert.equal(crossSessionOf("see <cross-session-message> for the shape"), null);
});

test("Droppen's own marker is taken off its notice", () => {
  assert.equal(
    droppenNoticeOf("[Droppen] This session switched from account a to b."),
    "This session switched from account a to b.",
  );
  assert.equal(droppenNoticeOf("[Droppen]"), null, "a bare marker is not a notice");
  assert.equal(droppenNoticeOf("A [Droppen] notice quoted mid-line."), null);
});
