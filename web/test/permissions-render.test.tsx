import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window, document } = installJsdom();
(globalThis as Record<symbol, unknown>)[Symbol.for("undici.globalOrigin.1")] = new URL("http://device.test");
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { act } = await import("react");
const i18next = (await import("i18next")).default;
const { initReactI18next } = await import("react-i18next");
await i18next.use(initReactI18next).init({ lng: "en", resources: {}, keySeparator: false, nsSeparator: false, interpolation: { escapeValue: false } });
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const { MachineProvider } = await import("../src/lib/machines/context.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const registry = await import("../src/lib/machines/registry.js");
const { constantStore } = await import("../src/lib/machines/store.js");
const { PendingPermissions } = await import("../src/features/sessions/permission-card.js");
const { composerAnswersQuestion } = await import("../src/features/sessions/session-questions.js");
const {
  effectivePermission,
  hasRejectOption,
  pendingPermissionsOf,
  selectedOptionName,
} = await import("../src/features/sessions/session-permissions.js");
type Transport = import("../src/lib/transport/index.js").Transport;
type SessionPermission = import("../src/features/sessions/session-permissions.js").SessionPermission;

interface Sent {
  method: string;
  path: string;
  body: unknown;
}

const samplePermission = (overrides: Partial<SessionPermission> = {}): SessionPermission => ({
  requestId: "perm-1",
  toolCallId: "call-1",
  title: "rm -rf build",
  toolName: "shell",
  reason: "Deletes directory",
  input: '{"command": "rm -rf build"}',
  options: [
    { id: "once", name: "Allow once", kind: "allow_once" },
    { id: "deny", name: "Deny", kind: "reject_once" },
  ],
  state: "pending",
  selectedOptionId: null,
  answeredBy: null,
  answeredAt: null,
  createdAt: "2026-11-28T03:00:00.000Z",
  ...overrides,
});

let root: ReturnType<typeof createRoot> | null = null;
const container = document.body.appendChild(document.createElement("div"));
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
});

function mountWith(sent: Sent[], handler: (req: Request, body: unknown) => Response, permissions: SessionPermission[]) {
  const transport: Transport = {
    fetch: async (request: Request) => {
      const url = new URL(request.url);
      const body = request.method === "GET" ? null : await request.json().catch(() => null);
      sent.push({ method: request.method, path: url.pathname, body });
      return handler(request, body);
    },
    socketUrl: async (path) => path,
    openSocket: () => ({ readyState: 0, binaryType: "blob", send() {}, close() {}, onopen: null, onmessage: null, onclose: null }),
  };
  registry.setShell("local", LOCAL);
  registry.openMachine(LOCAL, transport, {
    info: constantStore({ key: LOCAL, name: "studio", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    hold: () => () => undefined,
    dispose: () => undefined,
  });
  const queries = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  root = createRoot(container);
  return act(async () => {
    root!.render(
      <QueryClientProvider client={queries}>
        <MachineProvider machine={LOCAL}>
          <PendingPermissions permissions={permissions} session={{ source: "acp", id: "sess-1" }} />
        </MachineProvider>
      </QueryClientProvider>,
    );
  });
}

const settle = (ms = 30) => act(async () => new Promise((resolve) => setTimeout(resolve, ms)));
const click = (el: HTMLElement | null) =>
  act(async () => {
    assert.ok(el, "button exists");
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });

test("permission helpers filter pending, detect reject options, and keep composer off question mode", () => {
  const pending = samplePermission();
  const resolved = samplePermission({ requestId: "perm-2", state: "resolved", selectedOptionId: "once" });
  assert.deepEqual(pendingPermissionsOf([pending, resolved]).map((p) => p.requestId), ["perm-1"]);
  assert.equal(hasRejectOption(pending.options), true);
  assert.equal(hasRejectOption([{ id: "once", name: "Allow once", kind: "allow_once" }]), false);
  assert.equal(selectedOptionName(resolved), "Allow once");
  assert.equal(effectivePermission(pending, { ...pending, state: "resolved", selectedOptionId: "once" })?.state, "resolved");

  // Pending permission sets resident.asking, so composerAnswersQuestion must stay false.
  assert.equal(
    composerAnswersQuestion({
      questions: [],
      permissions: [pending],
      row: { status: "running", resident: { phase: "working", queued: 0, background: 0, asking: true } } as never,
    }),
    false,
  );
});

test("clicking an option posts the decision and settles the permission card", async () => {
  const sent: Sent[] = [];
  const perm = samplePermission();
  await mountWith(
    sent,
    () =>
      Response.json({
        permission: {
          ...perm,
          state: "resolved",
          selectedOptionId: "once",
          answeredBy: "alice",
          answeredAt: "2026-11-28T03:00:05.000Z",
        },
      }),
    [perm],
  );
  assert.ok(document.querySelector('[data-permission="pending"]'));
  assert.equal(document.querySelector('[data-permission-cancel]'), null, "explicit reject option hides fallback cancel");
  await click(document.querySelector<HTMLElement>('[data-permission-option="once"]'));
  await settle();
  assert.equal(sent.length, 1);
  assert.equal(sent[0]!.path, "/api/sessions/acp/sess-1/permissions/perm-1");
  assert.deepEqual(sent[0]!.body, { optionId: "once" });
  assert.ok(document.querySelector('[data-settled="resolved"]'));
  assert.match(document.querySelector('[data-settled="resolved"]')!.textContent!, /Allow once.*alice/);
});

test("409 permission_settled settles the card with the winning decision without showing an error", async () => {
  const sent: Sent[] = [];
  const perm = samplePermission({
    options: [{ id: "once", name: "Allow once", kind: "allow_once" }],
  });
  await mountWith(
    sent,
    () =>
      Response.json(
        {
          error: "permission_settled",
          hint: "bob already decided this permission",
          permission: {
            ...perm,
            state: "resolved",
            selectedOptionId: "once",
            answeredBy: "bob",
            answeredAt: "2026-11-28T03:00:03.000Z",
          },
        },
        { status: 409 },
      ),
    [perm],
  );
  assert.ok(document.querySelector('[data-permission-cancel]'), "fallback cancel shown when no reject option");
  await click(document.querySelector<HTMLElement>('[data-permission-cancel]'));
  await settle();
  assert.deepEqual(sent[0]!.body, { cancel: true });
  assert.equal(document.querySelector('[data-signal="failed"]'), null, "race loss is not shown as an error");
  assert.match(document.querySelector('[data-settled="resolved"]')!.textContent!, /Allow once.*bob/);
});
