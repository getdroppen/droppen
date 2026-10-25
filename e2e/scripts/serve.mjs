// Playwright's `webServer` entry point: prepare the fixtures and start the
// multi-host HTTPS listener in this process (the tests and the hub log share
// the machine).
import { prepare } from "./prepare.mjs";

prepare();
await import("../server/start.mjs");
