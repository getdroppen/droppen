import type { i18n as I18n } from "i18next";
import type { Root } from "react-dom/client";
import type { Language } from "./match";

/** A language's translations, as one table (`lang-*.ts`), fetched on demand. */
export type LanguageChunk = () => Promise<{ default: Record<string, string> }>;

export interface LanguageLoader {
  /**
   * `lang`'s translations into i18next, fetched once: true once they are in,
   * false when the chunk did not load (logged; the next call fetches it
   * again). A language with no chunk (English: its keys are its words) has
   * nothing to fetch.
   */
  load(lang: Language): Promise<boolean>;
  /**
   * Show `lang` once its translations are in, and English when they cannot
   * be had. Until then the page stays in the language it is in, never half
   * translated; asked again meanwhile, only the language asked for last is
   * shown.
   */
  show(lang: Language): Promise<void>;
  /** Settles once the language asked for last shows (or English, if it could not load). */
  shown(): Promise<void>;
}

export function languageLoader(i18n: I18n, chunks: Partial<Record<Language, LanguageChunk>>): LanguageLoader {
  const loading = new Map<Language, Promise<boolean>>();
  let wanted: Language | null = null;
  let last: Promise<void> = Promise.resolve();

  const load = (lang: Language): Promise<boolean> => {
    const chunk = chunks[lang];
    if (!chunk) return Promise.resolve(true);
    let pending = loading.get(lang);
    if (pending === undefined) {
      pending = chunk()
        .then((tables) => {
          // Over what a page added to the language meanwhile (the public
          // pages' tables, added never overwriting): a key the app's tables
          // translate keeps their words, as when they were there first.
          i18n.addResourceBundle(lang, "translation", tables.default, true, true);
          return true;
        })
        .catch((err: unknown) => {
          loading.delete(lang);
          console.error(`the ${lang} translations did not load; showing English`, err);
          return false;
        });
      loading.set(lang, pending);
    }
    return pending;
  };

  const show = (lang: Language): Promise<void> => {
    wanted = lang;
    last = load(lang).then(async (loaded) => {
      if (wanted !== lang) return;
      const to = loaded ? lang : "en";
      if (i18n.language !== to) await i18n.changeLanguage(to);
    });
    return last;
  };

  return { load, show, shown: () => last };
}

/**
 * `root`, drawing nothing until `shown` settles: whatever is rendered into it
 * first appears in the page's language, never in English while that
 * language's translations are still on their way. Renders keep their order,
 * and happen however `shown` settles.
 */
export function rootAfter(root: Root, shown: Promise<void>): Root {
  const after = (then: () => void) => void shown.then(then, then);
  return {
    render: (children) => after(() => root.render(children)),
    unmount: () => after(() => root.unmount()),
  };
}
