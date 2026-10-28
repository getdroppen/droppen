// The demo the product-tour screenshots show: who `dev` is, what is in their
// projects, and every session on their machine, in English and in Chinese.
//
// Everything here is made up. Two kinds of session:
//
// * history: a finished conversation, written to disk before the server
//   starts (`seed.mjs`), as Claude Code / Codex write theirs;
// * live: a conversation the scripted `claude` (`cli/claude.mjs`) plays
//   through the real server once `stage.mjs` sends its first message. It
//   ends asking the user a question (`end: "ask"`) or in the middle of a tool
//   call that does not return (`end: "run"`).
//
// A step is one of:
//   { user }                     the user's message
//   { say }                      the agent's text
//   { tool, input, result?, error?, patch? }
//                                a tool call and its result (Edit/Write may
//                                carry `patch`, the diff Claude Code records)
//   { ask }                      AskUserQuestion, left unanswered
//   { title }                    the session's name (Claude's custom title)
// Codex sessions use { user } { say } { exec: cmd, output, exit? } { patch: envelope }.

export const HOME = "/Users/dev";
export const CODE = `${HOME}/code`;

/** Fixed ids, so the staging and capture scripts can address them. */
export const IDS = {
  ask: "5f3c2a1e-8b7d-4c6a-9e2f-1a3b5c7d9e01",
  work: "7a9e4b2c-1d3f-4e5a-8b6c-2d4f6a8c0e12",
  flaky: "2c4e6a8b-3d5f-4a7c-9e1b-3f5a7c9e1b23",
  postgres: "9b1d3f5a-4e6c-4b8d-a0f2-4a6c8e0a2c34",
  query: "4d6f8a0c-5e7a-4c9e-b1a3-5b7d9f1b3d45",
  docs: "0199f3a2-6b7c-7d8e-9f01-23456789abcd",
  deploy: "0199f1b4-2c3d-7e4f-8a9b-0c1d2e3f4a5b",
  export: "6e8a0c2e-7f9b-4e1d-8c3f-7d9f1b3d5f67",
  renovate: "0199e8c6-4d5e-7f60-8a1b-2c3d4e5f6a7b",
  node: "8f0b2d4f-9a1c-4f3e-9d5a-8e0a2c4e6a78",
  typos: "0199d2a8-6f70-7a81-9b2c-3d4e5f6a7b8c",
};

export const CLAUDE_MODEL = "claude-opus-5-5";
export const CODEX_MODEL = "gpt-5.5";

const L = (lang, en, zh) => (lang === "zh-CN" ? zh : en);

// ---------------------------------------------------------------- tool output

const ORDERS_TEST_FAIL = `> shop-api@1.4.0 test
> node --test test/orders

▶ POST /orders
  ✔ creates an order (3.1ms)
  ✔ rejects a missing API key (0.6ms)
  ✔ rejects an empty cart (0.5ms)
▶ POST /orders (5.8ms)
▶ GET /orders
  ✔ lists the caller's orders (1.9ms)
  ✔ paginates with a cursor (1.2ms)
  ✖ filters by status when the filter is empty (2.4ms)
    AssertionError: expected 200, got 500
        at test/orders/list.test.ts:41:12
▶ GET /orders (7.7ms)
ℹ tests 15
ℹ pass 14
ℹ fail 1
ℹ duration_ms 402.6`;

const ORDERS_TEST_PASS = `> shop-api@1.4.0 test
> node --test test/orders

▶ POST /orders
  ✔ creates an order (2.9ms)
  ✔ rejects a missing API key (0.5ms)
  ✔ rejects an empty cart (0.5ms)
▶ POST /orders (5.2ms)
▶ GET /orders
  ✔ lists the caller's orders (1.8ms)
  ✔ paginates with a cursor (1.1ms)
  ✔ filters by status when the filter is empty (0.9ms)
▶ GET /orders (6.0ms)
ℹ tests 15
ℹ pass 15
ℹ fail 0
ℹ duration_ms 388.1`;

const RATE_LIMIT_TEST_PASS = `> shop-api@1.4.0 test
> node --test test/orders test/middleware

▶ POST /orders
  ✔ creates an order (3.0ms)
  ✔ rejects a missing API key (0.5ms)
  ✔ answers 429 once a key is over its limit (4.2ms)
▶ POST /orders (9.1ms)
▶ rateLimit
  ✔ counts per API key, not per IP (1.3ms)
  ✔ resets when the window ends (1.0ms)
  ✔ sets Retry-After on a 429 (0.6ms)
  ✔ fails open when Redis is down (0.8ms)
▶ rateLimit (4.4ms)
ℹ tests 19
ℹ pass 19
ℹ fail 0
ℹ duration_ms 451.7`;

const ORDERS_ROUTE = `import { Router } from "express";
import { requireApiKey } from "../middleware/auth";
import { cancelOrder, createOrder, getOrder, listOrders } from "../orders/service";
import { parseStatus } from "../orders/status";

export const router = Router();

router.get("/", requireApiKey, async (req, res) => {
  const status = parseStatus(req.query.status);
  const page = await listOrders(req.apiKey.accountId, { status, cursor: req.query.cursor });
  res.json(page);
});

router.get("/:id", requireApiKey, getOrder);
router.post("/", requireApiKey, createOrder);
router.delete("/:id", requireApiKey, cancelOrder);`;

const APP_TS = `import express from "express";
import { config } from "./config";
import { redis } from "./lib/redis";
import { router as orders } from "./routes/orders";
import { router as health } from "./routes/health";

export const app = express();
app.use(express.json({ limit: "1mb" }));
app.use("/health", health);
app.use("/orders", orders);

export async function start() {
  await redis.connect();
  app.listen(config.port);
}`;

const RATE_LIMIT_TS = `import type { RequestHandler } from "express";
import { redis } from "../lib/redis";

interface Options {
  key: (req: Parameters<RequestHandler>[0]) => string;
  limit: number;
  windowSec?: number;
}

/** A fixed window per key, counted in Redis so every instance shares it. */
export function rateLimit({ key, limit, windowSec = 60 }: Options): RequestHandler {
  return async (req, res, next) => {
    const window = Math.floor(Date.now() / 1000 / windowSec);
    const counter = \`rl:\${key(req)}:\${window}\`;
    try {
      const used = await redis.incr(counter);
      if (used === 1) await redis.expire(counter, windowSec);
      if (used > limit) {
        res.set("Retry-After", String(windowSec));
        return res.status(429).json({ error: "rate_limited" });
      }
    } catch {
      // Redis down: serve the request rather than fail every order.
    }
    next();
  };
}`;

const numbered = (text, from = 1) =>
  text
    .split("\n")
    .map((line, i) => `${String(i + from).padStart(6)}→${line}`)
    .join("\n");

// ------------------------------------------------------------------ sessions

function askSession(lang) {
  const cwd = `${CODE}/shop-api`;
  const route = `${cwd}/src/routes/orders.ts`;
  return {
    id: IDS.ask,
    source: "claude",
    cwd,
    branch: "feat/orders-rate-limit",
    seen: true,
    startMinutesAgo: 48,
    history: [
      { title: L(lang, "Add rate limiting to the orders API", "给订单接口加限流") },
      {
        user: L(
          lang,
          "Run the orders tests, one of them has been failing on main since yesterday.",
          "跑一下订单相关的测试，main 上从昨天开始有一个一直挂。",
        ),
      },
      { say: L(lang, "Running them first to see what fails.", "我先跑一遍看看是哪个挂了。") },
      {
        tool: "Bash",
        input: { command: "npm test -- test/orders", description: L(lang, "Run the orders tests", "运行订单测试") },
        result: `Exit code 1\n${ORDERS_TEST_FAIL}`,
        error: true,
      },
      { tool: "Read", input: { file_path: route }, result: numbered(ORDERS_ROUTE) },
      {
        say: L(
          lang,
          'An empty `?status=` arrives as `""`, and `parseStatus("")` throws instead of meaning "no filter". Fixing it where the query is read:',
          '空的 `?status=` 传进来是 `""`，`parseStatus("")` 直接抛异常，而不是当作「不过滤」。在读取参数的地方修一下：',
        ),
      },
      {
        tool: "Edit",
        input: {
          file_path: route,
          old_string: "  const status = parseStatus(req.query.status);",
          new_string: "  const raw = req.query.status;\n  const status = raw ? parseStatus(raw) : undefined;",
        },
        result: `The file ${route} has been updated.`,
        patch: {
          start: 8,
          lines: [
            ' router.get("/", requireApiKey, async (req, res) => {',
            "-  const status = parseStatus(req.query.status);",
            "+  const raw = req.query.status;",
            "+  const status = raw ? parseStatus(raw) : undefined;",
            "   const page = await listOrders(req.apiKey.accountId, { status, cursor: req.query.cursor });",
          ],
        },
      },
      {
        tool: "Bash",
        input: { command: "npm test -- test/orders", description: L(lang, "Re-run the orders tests", "重新运行订单测试") },
        result: ORDERS_TEST_PASS,
      },
      {
        say: L(
          lang,
          "Fixed: an empty `status` now means no filter, and all 15 orders tests pass.",
          "修好了：空的 `status` 现在表示不过滤，订单的 15 个测试全部通过。",
        ),
      },
    ],
    live: {
      end: "ask",
      steps: [
        {
          user: L(
            lang,
            "Thanks. Now add rate limiting to POST /orders, one integration keeps hammering it. Per API key, please.",
            "好的。再给 POST /orders 加个限流吧，有个对接方一直在狂刷。按 API key 限。",
          ),
        },
        { say: L(lang, "I'll check how the middleware and Redis are wired first.", "我先看看中间件和 Redis 是怎么接的。") },
        {
          tool: "Grep",
          input: { pattern: "createClient|redis\\.", output_mode: "content" },
          result:
            'src/lib/redis.ts:1:import { createClient } from "redis";\nsrc/lib/redis.ts:4:export const redis = createClient({ url: config.redisUrl });\nsrc/sessions/store.ts:9:  await redis.set(key, JSON.stringify(session), { EX: ttl });',
        },
        { tool: "Read", input: { file_path: `${cwd}/src/app.ts` }, result: numbered(APP_TS) },
        {
          say: L(
            lang,
            "Redis is already shared by every API instance, so a fixed-window counter there limits each key across the whole fleet. Adding the middleware:",
            "Redis 已经被所有 API 实例共用，在里面做固定窗口计数，就能按 key 跨实例限流。先加中间件：",
          ),
        },
        {
          tool: "Write",
          input: { file_path: `${cwd}/src/middleware/rate-limit.ts`, content: RATE_LIMIT_TS },
          result: `File created successfully at: ${cwd}/src/middleware/rate-limit.ts`,
        },
        {
          tool: "Edit",
          input: {
            file_path: route,
            old_string: 'router.post("/", requireApiKey, createOrder);',
            new_string:
              'const limitOrders = rateLimit({ key: (req) => req.apiKey.id, limit: config.orderLimit });\nrouter.post("/", requireApiKey, limitOrders, createOrder);',
          },
          result: `The file ${route} has been updated.`,
          patch: {
            start: 14,
            lines: [
              ' router.get("/:id", requireApiKey, getOrder);',
              '-router.post("/", requireApiKey, createOrder);',
              "+const limitOrders = rateLimit({ key: (req) => req.apiKey.id, limit: config.orderLimit });",
              '+router.post("/", requireApiKey, limitOrders, createOrder);',
              ' router.delete("/:id", requireApiKey, cancelOrder);',
            ],
          },
        },
        {
          tool: "Bash",
          input: {
            command: "npm test -- test/orders test/middleware",
            description: L(lang, "Run the orders and middleware tests", "运行订单和中间件测试"),
          },
          result: RATE_LIMIT_TEST_PASS,
        },
        {
          say: L(
            lang,
            "The limiter is in and all 19 tests pass. One decision before I set the production default:",
            "限流已经接上，19 个测试全部通过。设置生产环境默认值之前，需要你定一下：",
          ),
        },
        {
          ask: {
            questions: [
              {
                question: L(
                  lang,
                  "How many POST /orders requests per API key should be allowed?",
                  "每个 API key 允许多少次 POST /orders 请求？",
                ),
                header: L(lang, "Limit", "限额"),
                multiSelect: false,
                options: [
                  {
                    label: L(lang, "60 per minute", "每分钟 60 次"),
                    description: L(lang, "Recommended: about 3× the busiest client's peak today", "推荐：约为目前最忙的客户端峰值的 3 倍"),
                  },
                  {
                    label: L(lang, "20 per minute", "每分钟 20 次"),
                    description: L(lang, "Strict; the nightly import would need its own key", "较严格；夜间批量导入需要单独的 key"),
                  },
                  {
                    label: L(lang, "300 per minute", "每分钟 300 次"),
                    description: L(lang, "Only stops runaway retry loops", "只拦截失控的重试循环"),
                  },
                ],
              },
            ],
          },
        },
      ],
    },
  };
}

function workSession(lang) {
  const cwd = `${CODE}/web`;
  const dir = `${cwd}/src/checkout`;
  return {
    id: IDS.work,
    source: "claude",
    cwd,
    branch: "feat/checkout-dark-mode",
    seen: true,
    startMinutesAgo: 6,
    history: [{ title: L(lang, "Dark mode for the checkout page", "结算页适配深色模式") }],
    live: {
      end: "run",
      steps: [
        {
          user: L(
            lang,
            "The checkout page ignores dark mode. Move its hard-coded colours to the theme tokens and check the contrast in both themes.",
            "结算页不跟随深色模式。把里面写死的颜色换成主题 token，两种主题下都检查一下对比度。",
          ),
        },
        { say: L(lang, "Finding the hard-coded colours first.", "先把写死的颜色找出来。") },
        {
          tool: "Grep",
          input: { pattern: "bg-white|text-gray-\\d+|#[0-9a-f]{6}", glob: "src/checkout/**", output_mode: "count" },
          result: "src/checkout/Summary.tsx:6\nsrc/checkout/PaymentForm.tsx:5\nsrc/checkout/AddressCard.tsx:3",
        },
        {
          tool: "Read",
          input: { file_path: `${dir}/Summary.tsx` },
          result: numbered(
            'export function Summary({ cart }: { cart: Cart }) {\n  return (\n    <section className="bg-white text-gray-900">\n      <h2>Order summary</h2>\n      <dl className="mt-4 space-y-2">',
          ),
        },
        {
          tool: "Edit",
          input: {
            file_path: `${dir}/Summary.tsx`,
            old_string: '<section className="bg-white text-gray-900">',
            new_string: '<section className="bg-surface text-fg">',
          },
          result: `The file ${dir}/Summary.tsx has been updated.`,
          patch: {
            start: 2,
            lines: [
              "   return (",
              '-    <section className="bg-white text-gray-900">',
              '+    <section className="bg-surface text-fg">',
              "       <h2>Order summary</h2>",
            ],
          },
        },
        {
          tool: "Edit",
          input: {
            file_path: `${dir}/PaymentForm.tsx`,
            old_string: 'className="border-gray-200 bg-white"',
            new_string: 'className="border-border bg-surface"',
          },
          result: `The file ${dir}/PaymentForm.tsx has been updated.`,
          patch: {
            start: 31,
            lines: [
              "       <input",
              '-        className="border-gray-200 bg-white"',
              '+        className="border-border bg-surface"',
              '         name="card"',
            ],
          },
        },
        {
          tool: "Edit",
          input: {
            file_path: `${dir}/AddressCard.tsx`,
            old_string: 'style={{ color: "#6b7280" }}',
            new_string: 'className="text-muted"',
          },
          result: `The file ${dir}/AddressCard.tsx has been updated.`,
          patch: {
            start: 12,
            lines: ['-    <p style={{ color: "#6b7280" }}>{line2}</p>', '+    <p className="text-muted">{line2}</p>'],
          },
        },
        {
          tool: "Bash",
          input: { command: "npm test -- src/checkout", description: L(lang, "Run the checkout tests", "运行结算页测试") },
          result:
            "> web@0.9.0 test\n> vitest run src/checkout\n\n ✓ src/checkout/Summary.test.tsx (5 tests) 41ms\n ✓ src/checkout/PaymentForm.test.tsx (8 tests) 63ms\n ✓ src/checkout/AddressCard.test.tsx (3 tests) 17ms\n\n Test Files  3 passed (3)\n      Tests  16 passed (16)\n   Duration  1.24s",
        },
        {
          say: L(
            lang,
            "Every colour is on a token now. Taking screenshots in both themes to check the contrast:",
            "颜色都换成 token 了。接下来在两种主题下截图，检查对比度：",
          ),
        },
        {
          tool: "Bash",
          input: {
            command: "npx playwright test checkout --project=light --project=dark",
            description: L(lang, "Screenshot checkout in both themes", "在两种主题下截图结算页"),
          },
        },
      ],
    },
  };
}

function flakySession(lang) {
  const cwd = `${CODE}/web`;
  const spec = `${cwd}/e2e/checkout.spec.ts`;
  return {
    id: IDS.flaky,
    source: "claude",
    cwd,
    branch: "main",
    seen: true,
    startMinutesAgo: 150,
    history: [
      { title: L(lang, "Fix the flaky checkout e2e test", "修复结算流程不稳定的 e2e 测试") },
      {
        user: L(
          lang,
          "checkout.spec.ts fails about one run in five on CI. Find out why and fix it properly, no retries.",
          "checkout.spec.ts 在 CI 上大概五次挂一次。查一下原因，好好修，别靠重试。",
        ),
      },
      {
        tool: "Read",
        input: { file_path: spec },
        result: numbered(
          'test("pays for a cart", async ({ page }) => {\n  await page.goto("/checkout");\n  await page.fill("#zip", "94107");\n  await page.click("text=Pay");\n  await expect(page.getByText("Thank you")).toBeVisible();\n});',
        ),
      },
      {
        say: L(
          lang,
          "The test clicks Pay while the shipping quote for the new ZIP code is still loading, so the order sometimes goes out with the old total and the server rejects it.",
          "测试在新邮编的运费还没算出来时就点了支付，所以订单有时带着旧的总价提交，被服务端拒掉了。",
        ),
      },
      {
        tool: "Edit",
        input: {
          file_path: spec,
          old_string: '  await page.click("text=Pay");',
          new_string:
            '  await expect(page.getByTestId("total")).toHaveText("$42.90");\n  await page.getByRole("button", { name: "Pay" }).click();',
        },
        result: `The file ${spec} has been updated.`,
        patch: {
          start: 3,
          lines: [
            '   await page.fill("#zip", "94107");',
            '-  await page.click("text=Pay");',
            '+  await expect(page.getByTestId("total")).toHaveText("$42.90");',
            '+  await page.getByRole("button", { name: "Pay" }).click();',
            '   await expect(page.getByText("Thank you")).toBeVisible();',
          ],
        },
      },
      {
        tool: "Bash",
        input: {
          command: "npx playwright test e2e/checkout.spec.ts --repeat-each=20",
          description: L(lang, "Run the test 20 times", "把这个测试连跑 20 次"),
        },
        result: "Running 20 tests using 4 workers\n\n  20 passed (14.2s)",
      },
      {
        say: L(
          lang,
          "Fixed. The test now waits for the updated total before paying, which is also what a person does. 20 runs in a row pass; no retry added.",
          "修好了。测试现在会等新的总价出来再点支付，和真人操作的顺序一致。连跑 20 次全部通过，没有加重试。",
        ),
      },
    ],
  };
}

function postgresSession(lang) {
  const cwd = `${CODE}/infra`;
  const tf = `${cwd}/terraform/staging/db.tf`;
  return {
    id: IDS.postgres,
    source: "claude",
    cwd,
    branch: "staging-pg16",
    seen: false,
    startMinutesAgo: 41,
    history: [
      { title: L(lang, "Upgrade staging to Postgres 16", "把 staging 升级到 Postgres 16") },
      {
        user: L(lang, "Bump staging's Postgres from 14 to 16. Plan first, then apply.", "把 staging 的 Postgres 从 14 升到 16。先 plan，再 apply。"),
      },
      { tool: "Read", input: { file_path: tf }, result: numbered('resource "aws_db_instance" "main" {\n  engine         = "postgres"\n  engine_version = "14.12"\n  instance_class = "db.t4g.medium"\n  parameter_group_name = aws_db_parameter_group.pg14.name') },
      {
        tool: "Edit",
        input: { file_path: tf, old_string: 'engine_version = "14.12"', new_string: 'engine_version = "16.4"' },
        result: `The file ${tf} has been updated.`,
        patch: {
          start: 2,
          lines: ['   engine         = "postgres"', '-  engine_version = "14.12"', '+  engine_version = "16.4"', '   instance_class = "db.t4g.medium"'],
        },
      },
      {
        tool: "Bash",
        input: { command: "terraform -chdir=terraform/staging plan -out=pg16.plan", description: L(lang, "Plan the upgrade", "生成升级计划") },
        result:
          "  # aws_db_instance.main will be updated in-place\n  ~ resource \"aws_db_instance\" \"main\" {\n      ~ engine_version       = \"14.12\" -> \"16.4\"\n      ~ parameter_group_name = \"staging-pg14\" -> \"staging-pg16\"\n    }\n\nPlan: 1 to add, 1 to change, 0 to destroy.",
      },
      {
        tool: "Bash",
        input: { command: "terraform -chdir=terraform/staging apply pg16.plan", description: L(lang, "Apply the plan", "执行升级") },
        result: "aws_db_parameter_group.pg16: Creating...\naws_db_parameter_group.pg16: Creation complete after 2s\naws_db_instance.main: Modifying...\naws_db_instance.main: Modifications complete after 11m42s\n\nApply complete! Resources: 1 added, 1 changed, 0 destroyed.",
      },
      {
        say: L(
          lang,
          "Staging is on Postgres 16.4. The apply took 12 minutes, with about 40 seconds of downtime during the restart. Smoke tests against staging pass. The old pg14 parameter group is still there; I'll remove it once you're happy with staging.",
          "staging 已经升级到 Postgres 16.4。apply 用了 12 分钟，重启期间大约中断了 40 秒，staging 的冒烟测试全部通过。旧的 pg14 参数组还留着，等你确认 staging 没问题后我再删。",
        ),
      },
    ],
  };
}

function querySession(lang) {
  const cwd = `${CODE}/shop-api`;
  return {
    id: IDS.query,
    source: "claude",
    cwd,
    branch: "main",
    seen: true,
    startMinutesAgo: 26 * 60,
    history: [
      { title: L(lang, "Speed up the orders list query", "优化订单列表查询") },
      { user: L(lang, "GET /orders takes over a second for big accounts. Can you make it fast?", "大客户调 GET /orders 要一秒多，能优化一下吗？") },
      {
        tool: "Bash",
        input: {
          command: 'psql "$DATABASE_URL" -c "EXPLAIN ANALYZE SELECT * FROM orders WHERE account_id = 42 ORDER BY created_at DESC LIMIT 50"',
          description: L(lang, "Explain the slow query", "分析慢查询"),
        },
        result:
          " Limit  (cost=48210.11..48210.24 rows=50) (actual time=1184.2..1184.3 rows=50 loops=1)\n   ->  Sort  (cost=48210.11..48551.37 rows=136504)\n         Sort Key: created_at DESC\n         ->  Seq Scan on orders  (rows=136504) (actual time=0.03..1102.7)\n               Filter: (account_id = 42)\n Execution Time: 1184.6 ms",
      },
      {
        tool: "Write",
        input: {
          file_path: `${cwd}/migrations/0042_orders_account_created_idx.sql`,
          content: "CREATE INDEX CONCURRENTLY orders_account_created_idx\n  ON orders (account_id, created_at DESC);",
        },
        result: `File created successfully at: ${cwd}/migrations/0042_orders_account_created_idx.sql`,
      },
      {
        say: L(
          lang,
          "Added an index on `(account_id, created_at DESC)`, created concurrently so the table stays writable. The same query now takes 0.4 ms instead of 1.18 s.",
          "加了 `(account_id, created_at DESC)` 索引，用 CONCURRENTLY 创建，建索引期间表仍可写入。同一个查询从 1.18 秒降到 0.4 毫秒。",
        ),
      },
    ],
  };
}

function docsSession(lang) {
  const cwd = `${CODE}/docs`;
  return {
    id: IDS.docs,
    source: "codex",
    cwd,
    seen: false,
    startMinutesAgo: 75,
    title: L(lang, "Document the orders endpoints", "补全订单接口文档"),
    history: [
      {
        user: L(
          lang,
          "The API reference has nothing on the orders endpoints yet. Write it from the shop-api routes, with an example for each.",
          "API 参考文档里还没有订单接口。照着 shop-api 的路由写一份，每个接口都配上示例。",
        ),
      },
      {
        exec: "rg -n \"router\\.(get|post)\" ../shop-api/src/routes/orders.ts",
        output: '8:router.get("/", requireApiKey, async (req, res) => {\n14:router.post("/", requireApiKey, createOrder);',
      },
      {
        patch: `*** Begin Patch\n*** Add File: api/orders.md\n+# Orders\n+\n+## List orders\n+\n+\`GET /orders?status=paid&cursor=…\`\n+\n+## Create an order\n+\n+\`POST /orders\` with an \`Idempotency-Key\` header.\n*** End Patch\n`,
      },
      { exec: "npx markdownlint-cli2 api/orders.md", output: "Linting: 1 file(s)\nSummary: 0 error(s)" },
      {
        say: L(
          lang,
          "Added `api/orders.md`: both endpoints, their parameters and errors, with a curl example each. markdownlint is clean.",
          "已新增 `api/orders.md`：两个接口的参数和错误码都写了，各配一个 curl 示例，markdownlint 检查通过。",
        ),
      },
    ],
  };
}

function deploySession(lang) {
  const cwd = `${CODE}/infra`;
  return {
    id: IDS.deploy,
    source: "codex",
    cwd,
    seen: true,
    startMinutesAgo: 200,
    title: L(lang, "Add a health check to the deploy script", "给部署脚本加健康检查"),
    history: [
      {
        user: L(
          lang,
          "deploy.sh reports success even when the new version never comes up. Make it wait for /health and roll back if it doesn't pass.",
          "deploy.sh 在新版本没起来的时候也报成功。让它等 /health 通过，不通过就回滚。",
        ),
      },
      {
        exec: "sed -n '20,40p' scripts/deploy.sh",
        output:
          'kubectl set image deploy/api api="$IMAGE"\nkubectl rollout status deploy/api --timeout=120s\necho "deployed $IMAGE"',
      },
      {
        patch: `*** Begin Patch\n*** Update File: scripts/deploy.sh\n@@\n kubectl set image deploy/api api="$IMAGE"\n-kubectl rollout status deploy/api --timeout=120s\n+if ! kubectl rollout status deploy/api --timeout=120s || ! wait_healthy "$API_URL/health" 60; then\n+  echo "health check failed, rolling back" >&2\n+  kubectl rollout undo deploy/api\n+  exit 1\n+fi\n echo "deployed $IMAGE"\n*** End Patch\n`,
      },
      { exec: "shellcheck scripts/deploy.sh && bash -n scripts/deploy.sh && echo ok", output: "ok" },
      {
        say: L(
          lang,
          "deploy.sh now waits up to 60 s for `/health` after the rollout and runs `kubectl rollout undo` if it never passes, exiting non-zero so CI marks the deploy as failed.",
          "deploy.sh 现在会在发布后最多等 60 秒检查 `/health`，一直不通过就执行 `kubectl rollout undo` 回滚，并以非零状态退出，让 CI 把这次部署标记为失败。",
        ),
      },
    ],
  };
}

function exportSession(lang) {
  const cwd = `${CODE}/web`;
  return {
    id: IDS.export,
    source: "claude",
    cwd,
    branch: "main",
    seen: true,
    startMinutesAgo: 2 * 24 * 60 + 95,
    history: [
      { title: L(lang, "CSV export for the admin orders page", "后台订单页支持导出 CSV") },
      {
        user: L(lang, "Add a CSV export button to the admin orders page, using the current filters.", "给后台订单页加一个导出 CSV 的按钮，按当前的筛选条件导出。"),
      },
      {
        tool: "Write",
        input: {
          file_path: `${cwd}/src/pages/admin/orders/export.ts`,
          content: 'export function toCsv(orders: Order[]): string {\n  const rows = orders.map((o) => [o.id, o.createdAt, o.status, o.total].join(","));\n  return ["id,created_at,status,total", ...rows].join("\\n");\n}',
        },
        result: `File created successfully at: ${cwd}/src/pages/admin/orders/export.ts`,
      },
      {
        say: L(
          lang,
          "Added an Export CSV button next to the filters; it exports exactly what the table shows, streamed in pages of 500 so large ranges don't time out.",
          "在筛选栏旁加了「导出 CSV」按钮，导出的内容和表格当前显示的一致，按每页 500 条分批拉取，范围大也不会超时。",
        ),
      },
    ],
  };
}

function renovateSession(lang) {
  const cwd = `${CODE}/infra`;
  return {
    id: IDS.renovate,
    source: "codex",
    cwd,
    seen: true,
    startMinutesAgo: 3 * 24 * 60 + 40,
    title: L(lang, "Set up Renovate for dependency updates", "配置 Renovate 自动更新依赖"),
    history: [
      {
        user: L(lang, "Set up Renovate for all four repos: group minor updates weekly, never automerge majors.", "给四个仓库都配上 Renovate：小版本更新每周合成一个 PR，大版本绝不自动合并。"),
      },
      {
        patch: `*** Begin Patch\n*** Add File: renovate.json\n+{\n+  "extends": ["config:recommended"],\n+  "schedule": ["before 9am on monday"],\n+  "packageRules": [{ "matchUpdateTypes": ["minor", "patch"], "groupName": "weekly minor" }]\n+}\n*** End Patch\n`,
      },
      {
        say: L(
          lang,
          "Added `renovate.json`: minor and patch updates are grouped into one PR every Monday morning, and majors always wait for a review.",
          "已添加 `renovate.json`：小版本和补丁更新每周一上午合成一个 PR，大版本更新一律等人审核。",
        ),
      },
    ],
  };
}

function nodeSession(lang) {
  const cwd = `${CODE}/infra`;
  const ci = `${cwd}/.github/workflows/ci.yml`;
  return {
    id: IDS.node,
    source: "claude",
    cwd,
    branch: "main",
    seen: true,
    startMinutesAgo: 4 * 24 * 60 + 210,
    history: [
      { title: L(lang, "Move CI to Node 22", "CI 升级到 Node 22") },
      { user: L(lang, "Our CI still runs Node 18, which is end of life. Move every workflow to Node 22.", "CI 还在用 Node 18，已经停止维护了。把所有 workflow 都升到 Node 22。") },
      {
        tool: "Edit",
        input: { file_path: ci, old_string: "node-version: 18", new_string: "node-version: 22", replace_all: true },
        result: `The file ${ci} has been updated.`,
        patch: { start: 14, lines: ["       - uses: actions/setup-node@v4", "         with:", "-          node-version: 18", "+          node-version: 22"] },
      },
      {
        say: L(lang, "All three workflows now use Node 22, and the full CI run on this branch is green.", "三个 workflow 都换成了 Node 22，这个分支上完整跑了一遍 CI，全部通过。"),
      },
    ],
  };
}

function typosSession(lang) {
  const cwd = `${CODE}/docs`;
  return {
    id: IDS.typos,
    source: "codex",
    cwd,
    seen: true,
    startMinutesAgo: 5 * 24 * 60 + 30,
    title: L(lang, "Fix broken links in the getting-started guide", "修复入门指南里的失效链接"),
    history: [
      { user: L(lang, "Some links in the getting-started guide 404. Find and fix them.", "入门指南里有些链接打开是 404，找出来修好。") },
      { exec: "npx lychee --offline guides/getting-started.md", output: "✗ guides/getting-started.md: ../api/auth.md (file not found)\n✗ guides/getting-started.md: ../api/order.md (file not found)\n2 errors" },
      {
        patch: `*** Begin Patch\n*** Update File: guides/getting-started.md\n@@\n-See [authentication](../api/auth.md) and [orders](../api/order.md).\n+See [authentication](../api/authentication.md) and [orders](../api/orders.md).\n*** End Patch\n`,
      },
      { say: L(lang, "Fixed the two broken links; lychee reports no errors now.", "修好了两个失效链接，lychee 检查已无报错。") },
    ],
  };
}

export function sessions(lang) {
  return [
    askSession(lang),
    workSession(lang),
    postgresSession(lang),
    docsSession(lang),
    flakySession(lang),
    deploySession(lang),
    querySession(lang),
    exportSession(lang),
    renovateSession(lang),
    nodeSession(lang),
    typosSession(lang),
  ];
}

// ------------------------------------------------------------------ projects

/** The files of each project under ~/code (git repos with a short history). */
export function projects(lang) {
  const readme = L(
    lang,
    `# shop-api

The orders and checkout API behind the shop.

## Run it

\`\`\`sh
npm install
npm run dev      # http://localhost:3000
npm test
\`\`\`

## Endpoints

| Method | Path | What it does |
| ------ | ---- | ------------ |
| GET | \`/orders\` | List the caller's orders, newest first |
| POST | \`/orders\` | Create an order from a cart |
| GET | \`/health\` | Liveness for the load balancer |

Every request needs an \`Authorization: Bearer <api key>\` header.

## Configuration

| Variable | Default |
| -------- | ------- |
| \`PORT\` | \`3000\` |
| \`DATABASE_URL\` | \`postgres://localhost/shop\` |
| \`REDIS_URL\` | \`redis://localhost:6379\` |
`,
    `# shop-api

商城背后的订单与结算 API。

## 本地运行

\`\`\`sh
npm install
npm run dev      # http://localhost:3000
npm test
\`\`\`

## 接口

| 方法 | 路径 | 说明 |
| ---- | ---- | ---- |
| GET | \`/orders\` | 列出调用方的订单，最新的在前 |
| POST | \`/orders\` | 根据购物车创建订单 |
| GET | \`/health\` | 负载均衡的存活检查 |

所有请求都需要带 \`Authorization: Bearer <api key>\` 请求头。

## 配置

| 变量 | 默认值 |
| ---- | ------ |
| \`PORT\` | \`3000\` |
| \`DATABASE_URL\` | \`postgres://localhost/shop\` |
| \`REDIS_URL\` | \`redis://localhost:6379\` |
`,
  );
  return {
    "shop-api": {
      branch: "feat/orders-rate-limit",
      commits: [
        "chore: initial express app",
        "feat(orders): create and list orders",
        "feat(orders): cursor pagination",
        "test(orders): cover status filters",
        "perf(orders): index account_id, created_at",
        "fix(orders): an empty status filter means no filter",
      ],
      branchCommits: ["feat(orders): rate limit POST /orders per API key"],
      files: {
        "README.md": readme,
        "package.json": JSON.stringify(
          { name: "shop-api", version: "1.4.0", private: true, type: "module", scripts: { dev: "node --watch src/server.ts", test: "node --test" } },
          null,
          2,
        ) + "\n",
        "src/app.ts": APP_TS + "\n",
        "src/routes/orders.ts": ORDERS_ROUTE + "\n",
        "src/middleware/rate-limit.ts": RATE_LIMIT_TS + "\n",
        "src/lib/redis.ts": 'import { createClient } from "redis";\nimport { config } from "../config";\n\nexport const redis = createClient({ url: config.redisUrl });\n',
        "src/config.ts": 'export const config = {\n  port: Number(process.env.PORT ?? 3000),\n  redisUrl: process.env.REDIS_URL ?? "redis://localhost:6379",\n  orderLimit: Number(process.env.ORDER_LIMIT ?? 60),\n};\n',
        "migrations/0042_orders_account_created_idx.sql": "CREATE INDEX CONCURRENTLY orders_account_created_idx\n  ON orders (account_id, created_at DESC);\n",
        // A real suite: `npm test` in the terminal runs these.
        "test/orders/create.test.ts": nodeTests("POST /orders", ["creates an order", "rejects a missing API key", "rejects an empty cart", "rejects an unknown product", "keeps the idempotency key", "answers 429 once a key is over its limit"]),
        "test/orders/list.test.ts": nodeTests("GET /orders", ["lists the caller's orders", "paginates with a cursor", "filters by status", "filters by status when the filter is empty", "never shows another account's orders"]),
        "test/middleware/rate-limit.test.ts": nodeTests("rateLimit", ["counts per API key, not per IP", "resets when the window ends", "sets Retry-After on a 429", "fails open when Redis is down"]),
        "Dockerfile": "FROM node:22-slim\nWORKDIR /app\nCOPY package*.json ./\nRUN npm ci --omit=dev\nCOPY . .\nCMD [\"node\", \"src/server.ts\"]\n",
        "docker-compose.yml": "services:\n  postgres:\n    image: postgres:16\n  redis:\n    image: redis:7\n",
        "tsconfig.json": JSON.stringify({ compilerOptions: { target: "es2023", module: "nodenext", strict: true, noEmit: true } }, null, 2) + "\n",
        "CHANGELOG.md": "# Changelog\n\n## 1.4.0\n\n- Cursor pagination for GET /orders\n",
        "docs/architecture.md": "# Architecture\n\nExpress, Postgres for orders, Redis for sessions and rate limits.\n",
        "scripts/seed-dev-data.ts": "// Fills a local database with a few hundred orders.\n",
        "src/server.ts": 'import { start } from "./app";\n\nawait start();\n',
        "src/routes/health.ts": 'import { Router } from "express";\n\nexport const router = Router();\nrouter.get("/", (_req, res) => res.json({ ok: true }));\n',
        ".gitignore": "node_modules/\n.env\n",
        ".env.example": "DATABASE_URL=postgres://localhost/shop\nREDIS_URL=redis://localhost:6379\n",
      },
    },
    web: {
      branch: "feat/checkout-dark-mode",
      commits: ["chore: vite + react", "feat(checkout): address and payment steps", "fix(e2e): wait for the shipping total before paying"],
      branchCommits: [],
      files: {
        "README.md": "# web\n\nThe shop's storefront (Vite + React).\n",
        "package.json": JSON.stringify({ name: "web", version: "0.9.0", private: true, type: "module", scripts: { dev: "vite", test: "vitest run" } }, null, 2) + "\n",
        "src/checkout/Summary.tsx": 'export function Summary({ cart }: { cart: Cart }) {\n  return (\n    <section className="bg-surface text-fg">\n      <h2>Order summary</h2>\n    </section>\n  );\n}\n',
        "e2e/checkout.spec.ts": 'import { expect, test } from "@playwright/test";\n',
        ".gitignore": "node_modules/\ndist/\n",
      },
    },
    infra: {
      branch: "staging-pg16",
      commits: ["chore: terraform for staging", "feat(deploy): roll out with kubectl", "feat(deploy): wait for /health and roll back"],
      branchCommits: ["chore(staging): postgres 16.4"],
      files: {
        "README.md": "# infra\n\nTerraform and deploy scripts for staging and production.\n",
        "terraform/staging/db.tf": 'resource "aws_db_instance" "main" {\n  engine         = "postgres"\n  engine_version = "16.4"\n  instance_class = "db.t4g.medium"\n}\n',
        "scripts/deploy.sh": '#!/usr/bin/env bash\nset -euo pipefail\nkubectl set image deploy/api api="$IMAGE"\n',
      },
    },
    docs: {
      branch: "main",
      commits: ["docs: getting started", "docs: authentication", "docs(api): orders"],
      branchCommits: [],
      files: {
        "README.md": "# docs\n\nThe developer documentation site.\n",
        "api/orders.md": "# Orders\n\n## List orders\n\n`GET /orders`\n\n## Create an order\n\n`POST /orders`\n",
      },
    },
  };
}

function nodeTests(suite, names) {
  const body = names.map((name) => `  test(${JSON.stringify(name)}, async () => {\n    await settle();\n    assert.ok(true);\n  });`).join("\n");
  return `import assert from "node:assert/strict";
import { describe, test } from "node:test";

const settle = () => new Promise((resolve) => setTimeout(resolve, 2 + Math.random() * 6));

describe(${JSON.stringify(suite)}, () => {
${body}
});
`;
}

/** What is typed into the new-session composer. */
export function newSessionDraft(lang) {
  return L(
    lang,
    "Add an index on orders(created_at) and backfill it without locking the table",
    "给 orders(created_at) 加索引，回填数据时不要锁表",
  );
}

/** Screen and terminal names. */
export function names(lang) {
  return {
    screen: L(lang, "Checkout", "结算页"),
    terminal: "shop-api",
    devTerminal: "web",
  };
}
