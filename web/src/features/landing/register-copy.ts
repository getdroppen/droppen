import i18n from "@/i18n";

/**
 * A page's words, registered where they are written: every page's table loads with the
 * page's own chunk, and a page that imports it cannot forget to merge it.
 *
 * `deep` and `overwrite: false`: a key the app already translates keeps the app's wording,
 * and a key two tables both carry keeps the first one loaded. The pages' keys are the
 * English text, so a table never has to know which order the tables arrive in.
 */
export function registerCopy(bundles: Record<string, Record<string, string>>): void {
  for (const [lng, bundle] of Object.entries(bundles)) i18n.addResourceBundle(lng, "translation", bundle, true, false);
}
