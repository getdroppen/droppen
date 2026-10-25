// The acceptance test for the test suite: break one deployment property at a
// time and require the spec that owns it to fail. A suite that stays green
// when the marker, the CORS origin, the site policy or the API base is wrong
// is not testing the deployment.
//
//   node scripts/mutation.mjs                 all four, chromium
//   node scripts/mutation.mjs --only cors     one
//
// Exit 0 only when every mutation makes its spec fail.
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

const CASES = {
  marker: { spec: "tests/app.spec.ts", grep: "loads the cloud build with the api. marker" },
  cors: { spec: "tests/app.spec.ts", grep: "loads the cloud build with the api. marker" },
  "site-policy": { spec: "tests/site.spec.ts", grep: "Permissions-Policy denies WebAuthn" },
  // The app's own request: the test evaluates a fetch against the marker's
  // origin, so a wrong base means the sign-up config never reaches the hub.
  "api-base": { spec: "tests/app.spec.ts", grep: "loads the cloud build with the api. marker" },
};

const only = (() => {
  const at = process.argv.indexOf("--only");
  return at >= 0 ? process.argv[at + 1] : null;
})();
if (only && !(only in CASES)) {
  console.error(`mutation: --only must be one of ${Object.keys(CASES).join(", ")}`);
  process.exit(2);
}

let caught = 0;
for (const [name, testCase] of Object.entries(CASES)) {
  if (only && name !== only) continue;
  console.log(`\n=== mutation ${name}: ${testCase.spec} --grep "${testCase.grep}" (must fail) ===`);
  const result = spawnSync(
    "npx",
    ["playwright", "test", "--project=chromium", testCase.spec, "--grep", testCase.grep, "--reporter=line"],
    {
      cwd: resolve(import.meta.dirname, ".."),
      env: { ...process.env, E2E_MUTATION: name, CI: process.env.CI ?? "" },
      stdio: "inherit",
    },
  );
  if (result.status === 0) {
    console.error(`mutation ${name}: NOT CAUGHT — the spec passed on a broken deployment`);
  } else {
    console.log(`mutation ${name}: caught (playwright exited ${result.status})`);
    caught++;
  }
}
const total = only ? 1 : Object.keys(CASES).length;
console.log(`\nmutation: ${caught}/${total} caught`);
if (caught !== total) process.exit(1);
