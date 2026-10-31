/**
 * The browsers the build writes for (`build.target`): Vite's default
 * (`"modules"`) with ES2021 and Firefox 79 in place of ES2020 and Firefox 78,
 * the first of each with logical assignment (`a ||= b`), so esbuild never
 * lowers it.
 *
 * Lowered and then minified, esbuild turns xterm.js's `let r; f(r ||= {})`
 * into `f(void 0 || (i = {}))` without the `let`, and that assignment to an
 * undeclared name throws in a module. xterm.js does this to answer DECRQM
 * (`CSI ? 12 $ p`, which vim asks at startup), so the throw stopped the
 * terminal's parser and nothing after it was painted (#444). The dev server
 * does not lower, which is why only a build had it.
 */
export const BUILD_TARGET = ["es2021", "chrome87", "edge88", "firefox79", "safari14"];
