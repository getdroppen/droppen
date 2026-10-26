// The app's own i18n (`src/i18n/index.ts`) with every language's translations
// in, for the tests that read them. A page fetches a language's chunk when it
// first shows that language, so `changeLanguage` alone would find none here.
// Imported where the test imported the module itself, after whatever globals
// it reads at load (the stored language, the browser's).
const app = await import("../src/i18n/index.js");
await Promise.all((["zh-CN", "zh-TW", "ja"] as const).map((lang) => app.loadLanguage(lang)));
// And in the language the page starts in, as its first frame is.
await app.languageShown();

export default app.default;
