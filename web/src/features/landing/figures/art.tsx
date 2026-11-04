import type { Flat, LineTone, SolidPaths, Tone } from "./iso-kit";
import { Dots, FaceText, Line, Solid } from "./draw";

/*
 * The static half of a figure is plain data: the geometry module builds a
 * painter's-ordered list of items, and this renders it. Anything that moves is
 * a separate element with a ref, laid over the static list by its figure.
 */

export type ArtItem =
  | { kind: "solid"; paths: SolidPaths; tone?: Tone; crease?: Tone | "none"; lit?: boolean }
  | { kind: "line"; d: string; tone?: LineTone; dashed?: boolean; dotted?: boolean; free?: boolean; width?: number }
  | { kind: "dots"; points: Array<Flat | [number, number, number]>; size?: number; tone?: Tone }
  | { kind: "halo"; d: string }
  | { kind: "text"; transform: string; text: string; size?: number; tone?: Tone; anchor?: "start" | "middle" | "end" };

export function Art({ items }: { items: ArtItem[] }) {
  return (
    <>
      {items.map((item, index) => {
        switch (item.kind) {
          case "solid":
            return <Solid key={index} paths={item.paths} tone={item.tone} crease={item.crease} lit={item.lit} />;
          case "line":
            return <Line key={index} d={item.d} tone={item.tone} dashed={item.dashed} dotted={item.dotted} free={item.free} width={item.width} />;
          case "dots":
            return <Dots key={index} points={item.points} size={item.size} tone={item.tone} />;
          case "halo":
            return <path key={index} className="iso-halo" d={item.d} />;
          case "text":
            return (
              <FaceText key={index} transform={item.transform} size={item.size} tone={item.tone} anchor={item.anchor}>
                {item.text}
              </FaceText>
            );
        }
      })}
    </>
  );
}
