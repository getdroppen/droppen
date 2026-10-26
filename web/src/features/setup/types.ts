/*
 * The wire shapes of `/api/setup/*` the first-run guide reads and writes (the
 * server's `crate::setup`), under the names the guide uses for them. All of
 * them are the generated schema's, except the sign-in modes, which the server
 * takes as a string and checks against its whitelist.
 */
import type { components } from "@/api/schema";

type Schemas = components["schemas"];

/** Where a step of the guide stands. */
export type StepState = Schemas["StepState"];

/** The four steps, in order: 01 AI tools, 02 where to use it, 03 first session, 04 finishing up. */
export type StepId = keyof Schemas["StepStates"];

/** `GET/PUT /api/setup/progress`: how far this device's guide got. `flow` and `autoOpen` are the server's. */
export type SetupProgress = Schemas["SetupProgress"];

export type ToolId = NonNullable<Schemas["ToolId"]>;

/** A running or last finished install, update or sign-in of one tool. */
export type SetupTask = NonNullable<Schemas["SetupTask"]>;

/** One AI tool as this device has it (`GET /api/setup/tools`). */
export type SetupTool = Schemas["Tool"];

/** `GET /api/setup/tools`, `POST /api/setup/tools/rescan`. */
export type ToolsReport = Schemas["ToolsReport"];

/** How a tool signs in (`POST /api/setup/tools/{tool}/login {mode}`). */
export type LoginMode = "claudeai" | "console" | "sso" | "chatgpt" | "device" | "apikey";

/**
 * One whitelisted command (`GET /api/setup/commands`): `command` is exactly
 * what runs, shown verbatim; a method this device cannot use says why.
 */
export type SetupCommand = Schemas["SetupCommand"];

/** A folder sessions ran in on this device (`GET /api/setup/projects/recent`): names and paths only, never content. */
export type RecentFolder = Schemas["RecentFolder"];

