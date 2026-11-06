import { motion, useReducedMotion } from "motion/react";
import { cn } from "@/lib/cn";

/*
 * The marks of the agents Droppen lists, thrown around the hero's action like
 * confetti when the page opens: the tools whose sessions the app follows.
 *
 * The marks name the tools and belong to their makers (OpenAI, Anthropic,
 * Google, Anysphere, GitHub, Codeium, the Cline and opencode projects, Warp);
 * Droppen ships none of them. The paths are the makers' own, taken from
 * Simple Icons (CC0, `simpleicons.org`) for Claude Code, Cursor, GitHub
 * Copilot, Gemini, Windsurf, Cline, Warp and opencode, from OpenAI's 2025
 * symbol (Wikimedia Commons) for OpenAI, and from the mark on
 * `antigravity.google` (its blue silhouette) for Antigravity. Monochrome marks
 * take the page's ink so they stay legible in both themes; the others keep the
 * maker's colour.
 *
 * OpenAI's blossom is one mark for ChatGPT and Codex, its two products here.
 */

type Mark = {
  name: string;
  viewBox: string;
  /** The maker's colour, or `ink` to follow the page (a black mark turns white in the dark theme). */
  color: string | "ink";
  path: string;
};

const MARKS: readonly Mark[] = [
  {
    name: "OpenAI",
    viewBox: "1.68 1.75 16.65 16.5",
    color: "ink",
    path:
      "M11.248 18.25q-.825 0-1.568-.314a4.3 4.3 0 0 1-1.32-.874 4 4 0 0 1-1.304.214 4 4 0 0 1-2.046-.544 4.27 4.27 0 0 1-1.518-1.485 4 4 0 0 1-.56-2.095q0-.48.131-1.04A4.4 4.4 0 0 1 2.04 10.71a4.07 4.07 0 0 1 .017-3.4 4.2 4.2 0 0 1 1.056-1.418 3.8 3.8 0 0 1 1.6-.842 3.9 3.9 0 0 1 .76-1.683q.593-.759 1.451-1.188a4.04 4.04 0 0 1 1.832-.429q.825 0 1.567.313.742.314 1.32.875a4 4 0 0 1 1.304-.215q1.106 0 2.046.545a4.14 4.14 0 0 1 1.501 1.485q.578.941.578 2.095 0 .48-.132 1.04.66.61 1.023 1.419.363.792.363 1.666 0 .892-.38 1.717a4.3 4.3 0 0 1-1.072 1.435 3.8 3.8 0 0 1-1.584.825 3.8 3.8 0 0 1-.775 1.683 4.06 4.06 0 0 1-1.436 1.188 4.04 4.04 0 0 1-1.832.429m-4.076-2.062q.825 0 1.435-.347l3.103-1.782a.36.36 0 0 0 .164-.313v-1.42L7.881 14.62a.67.67 0 0 1-.726 0l-3.118-1.798a.5.5 0 0 1-.017.115v.198q0 .841.396 1.551.413.693 1.139 1.089a3.2 3.2 0 0 0 1.617.412m.165-2.69a.4.4 0 0 0 .181.05q.083 0 .165-.05l1.238-.71-3.977-2.31a.7.7 0 0 1-.363-.643v-3.58q-.825.362-1.32 1.122a2.9 2.9 0 0 0-.495 1.65q0 .809.413 1.55.412.743 1.072 1.123zm3.91 3.663q.875 0 1.585-.396a2.96 2.96 0 0 0 1.534-2.64v-3.564a.32.32 0 0 0-.165-.297l-1.254-.726v4.604a.7.7 0 0 1-.363.643l-3.119 1.799a3 3 0 0 0 1.783.577m.627-6.039V8.878L10.01 7.822 8.129 8.878v2.244l1.881 1.056zM7.057 5.859a.7.7 0 0 1 .363-.644l3.119-1.798a3 3 0 0 0-1.782-.578q-.874 0-1.584.396A2.96 2.96 0 0 0 6.05 4.324a3.07 3.07 0 0 0-.396 1.551v3.547q0 .199.165.314l1.237.726zm8.383 7.887q.825-.364 1.303-1.123.495-.758.495-1.65a3.15 3.15 0 0 0-.412-1.55q-.413-.743-1.073-1.123l-3.086-1.782q-.099-.065-.181-.049a.3.3 0 0 0-.165.05l-1.238.692 3.993 2.327a.6.6 0 0 1 .264.264.64.64 0 0 1 .1.363zm-3.317-8.382a.63.63 0 0 1 .726 0l3.135 1.831v-.297q0-.792-.396-1.501a2.86 2.86 0 0 0-1.105-1.155q-.71-.43-1.65-.43-.825 0-1.436.347L8.294 5.941a.36.36 0 0 0-.165.314v1.418z",
  },
  {
    name: "Claude Code",
    viewBox: "0 0 24 24",
    color: "#D97757",
    path:
      "m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z",
  },
  {
    name: "Cursor",
    viewBox: "0 0 24 24",
    color: "ink",
    path:
      "M11.503.131 1.891 5.678a.84.84 0 0 0-.42.726v11.188c0 .3.162.575.42.724l9.609 5.55a1 1 0 0 0 .998 0l9.61-5.55a.84.84 0 0 0 .42-.724V6.404a.84.84 0 0 0-.42-.726L12.497.131a1.01 1.01 0 0 0-.996 0M2.657 6.338h18.55c.263 0 .43.287.297.515L12.23 22.918c-.062.107-.229.064-.229-.06V12.335a.59.59 0 0 0-.295-.51l-9.11-5.257c-.109-.063-.064-.23.061-.23",
  },
  {
    name: "GitHub Copilot",
    viewBox: "0 0 24 24",
    color: "ink",
    path:
      "M23.922 16.997C23.061 18.492 18.063 22.02 12 22.02 5.937 22.02.939 18.492.078 16.997A.641.641 0 0 1 0 16.741v-2.869a.883.883 0 0 1 .053-.22c.372-.935 1.347-2.292 2.605-2.656.167-.429.414-1.055.644-1.517a10.098 10.098 0 0 1-.052-1.086c0-1.331.282-2.499 1.132-3.368.397-.406.89-.717 1.474-.952C7.255 2.937 9.248 1.98 11.978 1.98c2.731 0 4.767.957 6.166 2.093.584.235 1.077.546 1.474.952.85.869 1.132 2.037 1.132 3.368 0 .368-.014.733-.052 1.086.23.462.477 1.088.644 1.517 1.258.364 2.233 1.721 2.605 2.656a.841.841 0 0 1 .053.22v2.869a.641.641 0 0 1-.078.256Zm-11.75-5.992h-.344a4.359 4.359 0 0 1-.355.508c-.77.947-1.918 1.492-3.508 1.492-1.725 0-2.989-.359-3.782-1.259a2.137 2.137 0 0 1-.085-.104L4 11.746v6.585c1.435.779 4.514 2.179 8 2.179 3.486 0 6.565-1.4 8-2.179v-6.585l-.098-.104s-.033.045-.085.104c-.793.9-2.057 1.259-3.782 1.259-1.59 0-2.738-.545-3.508-1.492a4.359 4.359 0 0 1-.355-.508Zm2.328 3.25c.549 0 1 .451 1 1v2c0 .549-.451 1-1 1-.549 0-1-.451-1-1v-2c0-.549.451-1 1-1Zm-5 0c.549 0 1 .451 1 1v2c0 .549-.451 1-1 1-.549 0-1-.451-1-1v-2c0-.549.451-1 1-1Zm3.313-6.185c.136 1.057.403 1.913.878 2.497.442.544 1.134.938 2.344.938 1.573 0 2.292-.337 2.657-.751.384-.435.558-1.15.558-2.361 0-1.14-.243-1.847-.705-2.319-.477-.488-1.319-.862-2.824-1.025-1.487-.161-2.192.138-2.533.529-.269.307-.437.808-.438 1.578v.021c0 .265.021.562.063.893Zm-1.626 0c.042-.331.063-.628.063-.894v-.02c-.001-.77-.169-1.271-.438-1.578-.341-.391-1.046-.69-2.533-.529-1.505.163-2.347.537-2.824 1.025-.462.472-.705 1.179-.705 2.319 0 1.211.175 1.926.558 2.361.365.414 1.084.751 2.657.751 1.21 0 1.902-.394 2.344-.938.475-.584.742-1.44.878-2.497Z",
  },
  {
    name: "Gemini",
    viewBox: "0 0 24 24",
    color: "#8E75B2",
    path:
      "M11.04 19.32Q12 21.51 12 24q0-2.49.93-4.68.96-2.19 2.58-3.81t3.81-2.55Q21.51 12 24 12q-2.49 0-4.68-.93a12.3 12.3 0 0 1-3.81-2.58 12.3 12.3 0 0 1-2.58-3.81Q12 2.49 12 0q0 2.49-.96 4.68-.93 2.19-2.55 3.81a12.3 12.3 0 0 1-3.81 2.58Q2.49 12 0 12q2.49 0 4.68.96 2.19.93 3.81 2.55t2.55 3.81",
  },
  {
    name: "Windsurf",
    viewBox: "0 0 24 24",
    color: "ink",
    path:
      "M23.55 5.067c-1.2038-.002-2.1806.973-2.1806 2.1765v4.8676c0 .972-.8035 1.7594-1.7597 1.7594-.568 0-1.1352-.286-1.4718-.7659l-4.9713-7.1003c-.4125-.5896-1.0837-.941-1.8103-.941-1.1334 0-2.1533.9635-2.1533 2.153v4.8957c0 .972-.7969 1.7594-1.7596 1.7594-.57 0-1.1363-.286-1.4728-.7658L.4076 5.1598C.2822 4.9798 0 5.0688 0 5.2882v4.2452c0 .2147.0656.4228.1884.599l5.4748 7.8183c.3234.462.8006.8052 1.3509.9298 1.3771.313 2.6446-.747 2.6446-2.0977v-4.893c0-.972.7875-1.7593 1.7596-1.7593h.003a1.798 1.798 0 0 1 1.4718.7658l4.9723 7.0994c.4135.5905 1.05.941 1.8093.941 1.1587 0 2.1515-.9645 2.1515-2.153v-4.8948c0-.972.7875-1.7594 1.7596-1.7594h.194a.22.22 0 0 0 .2204-.2202v-4.622a.22.22 0 0 0-.2203-.2203Z",
  },
  {
    name: "Cline",
    viewBox: "0 0 24 24",
    color: "ink",
    path:
      "m23.365 13.556-1.442-2.895V8.994c0-2.764-2.218-5.002-4.954-5.002h-2.464c.178-.367.276-.779.276-1.213A2.77 2.77 0 0 0 12.018 0a2.77 2.77 0 0 0-2.763 2.779c0 .434.098.846.276 1.213H7.067c-2.736 0-4.954 2.238-4.954 5.002v1.667L.64 13.549c-.149.29-.149.636 0 .927l1.472 2.855v1.667C2.113 21.762 4.33 24 7.067 24h9.902c2.736 0 4.954-2.238 4.954-5.002V17.33l1.44-2.865c.143-.286.143-.622.002-.91m-12.854 2.36a2.27 2.27 0 0 1-2.261 2.273 2.27 2.27 0 0 1-2.261-2.273v-4.042A2.27 2.27 0 0 1 8.249 9.6a2.267 2.267 0 0 1 2.262 2.274zm7.285 0a2.27 2.27 0 0 1-2.26 2.273 2.27 2.27 0 0 1-2.262-2.273v-4.042A2.267 2.267 0 0 1 15.535 9.6a2.267 2.267 0 0 1 2.261 2.274z",
  },
  {
    name: "Warp",
    viewBox: "0 0 24 24",
    color: "#01A4FF",
    path:
      "M12.035 2.723h9.253A2.712 2.712 0 0 1 24 5.435v10.529a2.712 2.712 0 0 1-2.712 2.713H8.047Zm-1.681 2.6L6.766 19.677h5.598l-.399 1.6H2.712A2.712 2.712 0 0 1 0 18.565V8.036a2.712 2.712 0 0 1 2.712-2.712Z",
  },
  {
    name: "opencode",
    viewBox: "0 0 24 24",
    color: "ink",
    path:
      "M22 24H2V0h20zM17 4.8H7v14.4h10z",
  },
  {
    name: "Antigravity",
    viewBox: "5.8 14.4 99.6 86.8",
    color: "#3186FF",
    path:
      "M89.6992 93.695C94.3659 97.195 101.366 94.8617 94.9492 88.445C75.6992 69.7783 79.7825 18.445 55.8659 18.445C31.9492 18.445 36.0325 69.7783 16.7825 88.445C9.78251 95.445 17.3658 97.195 22.0325 93.695C40.1159 81.445 38.9492 59.8617 55.8659 59.8617C72.7825 59.8617 71.6159 81.445 89.6992 93.695Z",
  },
];

/**
 * Where each mark lands, as a share of the field's width and height, with its
 * size in px and its tilt. The middle of the field is the action's: the marks
 * ring it, and `wideOnly` ones need the side room the desktop hero has.
 */
const SPOTS: readonly { mark: number; left: number; top: number; size: number; rotate: number; wideOnly?: boolean }[] = [
  { mark: 0, left: 5, top: 40, size: 46, rotate: -12, wideOnly: true },
  { mark: 1, left: 10, top: 70, size: 38, rotate: 9, wideOnly: true },
  { mark: 2, left: 16, top: 90, size: 24, rotate: -16 },
  { mark: 3, left: 88, top: 62, size: 40, rotate: 7, wideOnly: true },
  { mark: 4, left: 95, top: 34, size: 32, rotate: -10, wideOnly: true },
  { mark: 5, left: 63, top: 14, size: 30, rotate: 14 },
  { mark: 6, left: 40, top: 5, size: 34, rotate: -8 },
  { mark: 7, left: 68, top: 91, size: 28, rotate: -14 },
  { mark: 8, left: 21, top: 18, size: 24, rotate: 10 },
  { mark: 9, left: 79, top: 6, size: 46, rotate: 6 },
];

/** The paper the burst also throws: small squares in the marks' colours, between them. */
const BITS: readonly { left: number; top: number; size: number; rotate: number; color: string | "ink"; wideOnly?: boolean }[] = [
  { left: 14, top: 26, size: 7, rotate: 22, color: "#D97757" },
  { left: 7, top: 84, size: 8, rotate: -18, color: "#8E75B2", wideOnly: true },
  { left: 29, top: 22, size: 5, rotate: 12, color: "ink", wideOnly: true },
  { left: 52, top: 12, size: 6, rotate: -24, color: "#3186FF" },
  { left: 71, top: 24, size: 5, rotate: 16, color: "#D97757" },
  { left: 76, top: 86, size: 8, rotate: -12, color: "#01A4FF" },
  { left: 93, top: 76, size: 6, rotate: 28, color: "ink", wideOnly: true },
  { left: 97, top: 18, size: 7, rotate: -20, color: "#8E75B2", wideOnly: true },
  { left: 55, top: 90, size: 5, rotate: 18, color: "ink" },
  { left: 12, top: 56, size: 6, rotate: -14, color: "#3186FF", wideOnly: true },
  { left: 88, top: 90, size: 7, rotate: 24, color: "#D97757" },
  { left: 23, top: 74, size: 5, rotate: -9, color: "ink", wideOnly: true },
  { left: 77, top: 74, size: 5, rotate: 14, color: "#D97757", wideOnly: true },
];

/** One mark, in its maker's colour (or the page's ink). */
function Mark({ mark }: { mark: Mark }) {
  return (
    <svg
      aria-hidden="true"
      focusable="false"
      viewBox={mark.viewBox}
      fill="currentColor"
      className={cn("block h-full w-full", mark.color === "ink" && "text-fg")}
      style={mark.color === "ink" ? undefined : { color: mark.color }}
    >
      <path d={mark.path} />
    </svg>
  );
}

type Piece = {
  key: string;
  /** A mark draws its maker's shape; a bit is a square of the maker's colour on the paper. */
  mark?: Mark;
  color?: string | "ink";
  left: number;
  top: number;
  size: number;
  rotate: number;
  wideOnly: boolean;
};

/** Every mark and every bit, in the order they burst out. */
const PIECES: readonly Piece[] = [
  ...SPOTS.map((spot) => ({ key: MARKS[spot.mark]!.name, mark: MARKS[spot.mark]!, left: spot.left, top: spot.top, size: spot.size, rotate: spot.rotate, wideOnly: spot.wideOnly === true })),
  ...BITS.map((bit, index) => ({ key: `bit-${index}`, color: bit.color, left: bit.left, top: bit.top, size: bit.size, rotate: bit.rotate, wideOnly: bit.wideOnly === true })),
];

/**
 * The agent marks around the hero's action: on load they burst out of its
 * middle and settle, one after the next. Decoration only (the copy names the
 * agents), so the whole field is hidden from assistive technology and takes no
 * pointer; a reader who asked for less motion gets them already placed.
 */
export function AgentBurst({ className }: { className?: string }) {
  const reduced = useReducedMotion();
  const style = (piece: Piece) => ({ left: `${piece.left}%`, top: `${piece.top}%`, width: piece.size, height: piece.size });
  const body = (piece: Piece) =>
    piece.mark ? (
      <Mark mark={piece.mark} />
    ) : (
      <span className={cn("block size-full", piece.color === "ink" && "bg-fg")} style={piece.color === "ink" ? undefined : { backgroundColor: piece.color }} />
    );

  if (reduced) {
    return (
      <div aria-hidden="true" data-allow-color="" className={cn("pointer-events-none absolute inset-0", className)}>
        {PIECES.map((piece) => (
          <span key={piece.key} className={cn("absolute -translate-x-1/2 -translate-y-1/2", piece.wideOnly && "max-lg:hidden")} style={style(piece)}>
            <span className="block h-full w-full" style={{ transform: `rotate(${piece.rotate}deg)` }}>
              {body(piece)}
            </span>
          </span>
        ))}
      </div>
    );
  }

  return (
    <div aria-hidden="true" data-allow-color="" className={cn("pointer-events-none absolute inset-0", className)}>
      <motion.div
        className="absolute inset-0"
        style={{ transformOrigin: "50% 50%" }}
        initial={{ opacity: 0, scale: 0.5 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={{ type: "spring", stiffness: 200, damping: 24, mass: 0.8 }}
      >
        {PIECES.map((piece, index) => (
          <span key={piece.key} className={cn("absolute -translate-x-1/2 -translate-y-1/2", piece.wideOnly && "max-lg:hidden")} style={style(piece)}>
            <motion.span
              className="block h-full w-full"
              initial={{ opacity: 0, scale: 0.15, rotate: piece.rotate * 2 }}
              animate={{ opacity: 1, scale: 1, rotate: piece.rotate }}
              transition={{ type: "spring", stiffness: 420, damping: 25, mass: 0.6, delay: 0.06 + index * 0.018 }}
            >
              {body(piece)}
            </motion.span>
          </span>
        ))}
      </motion.div>
    </div>
  );
}
