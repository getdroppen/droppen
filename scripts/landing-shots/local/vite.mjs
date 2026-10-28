// Stands in for `vite` in the demo's web project, so `npm run dev` in the
// tour's terminal prints a dev server's start-up and the hot updates the
// agent's edits to the checkout page would cause. Then it idles like one.

const esc = (code) => (text) => `\x1b[${code}m${text}\x1b[0m`;
const [bold, dim, green, cyan, yellow] = [esc(1), esc(2), esc(32), esc(36), esc(33)];
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const clock = () => new Date().toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit", second: "2-digit" });

console.log("");
console.log(`  ${bold(green("VITE"))} ${green("v7.1.4")}  ${dim("ready in")} ${bold("412")} ${dim("ms")}`);
console.log("");
console.log(`  ${green("➜")}  ${bold("Local")}:   ${cyan(`http://localhost:${bold("5173")}/`)}`);
console.log(`  ${dim(green("➜"))}  ${dim(`${bold("Network")}: use ${bold("--host")} to expose`)}`);
console.log(`  ${dim(green("➜"))}  ${dim(`press ${bold("h + enter")} to show help`)}`);
for (const file of ["Summary.tsx", "PaymentForm.tsx", "AddressCard.tsx"]) {
  await sleep(700);
  console.log(`${dim(clock())} ${bold(cyan("[vite]"))} ${yellow("(client)")} ${green("hmr update")} ${dim(`/src/checkout/${file}`)}`);
}
await sleep(500);
console.log(`${dim(clock())} ${bold(cyan("[vite]"))} ${yellow("(client)")} ${green("hmr update")} ${dim("/src/styles/tokens.css")}`);
setInterval(() => {}, 1 << 30);
