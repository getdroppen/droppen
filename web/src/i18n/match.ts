// Which of the app's languages a BCP 47 tag names: no i18next here, so code
// that only needs the mapping (the plugin host) does not start the app's i18n.

/** A language the app speaks. */
export type Language = "en" | "zh-CN" | "zh-TW" | "ja";

/**
 * The language of one BCP 47 tag, if it is one the app speaks: Chinese in
 * Traditional script (`zh-Hant`, else Taiwan, Hong Kong or Macao) is zh-TW,
 * any other Chinese (`zh`, `zh-CN`, `zh-SG`, `zh-Hans`, …) zh-CN; `ja…` is
 * Japanese and `en…` English. The script, where a tag names one, decides
 * over the region (`zh-Hans-HK` is zh-CN). Anything else: `null`.
 */
export function matchLanguage(tag: string): Language | null {
  const [primary, ...rest] = tag.trim().replace(/_/g, "-").toLowerCase().split("-");
  if (primary === "en") return "en";
  if (primary === "ja") return "ja";
  if (primary !== "zh") return null;
  if (rest.includes("hant")) return "zh-TW";
  if (rest.includes("hans")) return "zh-CN";
  return rest.some((part) => part === "tw" || part === "hk" || part === "mo") ? "zh-TW" : "zh-CN";
}

/** The first of `tags`, in the reader's order, that the app speaks; English when none is. */
export function pickLanguage(tags: readonly (string | null | undefined)[]): Language {
  for (const tag of tags) {
    const lang = tag ? matchLanguage(tag) : null;
    if (lang) return lang;
  }
  return "en";
}
