import type { SVGProps } from "react";
import { siApple, siGithub, siLinux, type SimpleIcon } from "simple-icons";

/*
 * The makers' own marks, from Simple Icons (`simple-icons`, CC0-1.0,
 * simpleicons.org), not drawings of them: readers know these shapes at a
 * glance, and a generic "apple" or "github" glyph reads as neither.
 *
 * Apple and the Apple logo are trademarks of Apple Inc.; GitHub and the
 * Octocat are trademarks of GitHub, Inc.; Linux is a registered trademark of
 * Linus Torvalds. Droppen uses them only to name the platforms it runs on.
 * `currentColor` lets the surrounding ink colour them.
 */
type MarkProps = SVGProps<SVGSVGElement>;

function Mark({ icon, ...props }: { icon: SimpleIcon } & MarkProps) {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" focusable="false" {...props}>
      <path d={icon.path} />
    </svg>
  );
}

export function AppleMark(props: MarkProps) {
  return <Mark icon={siApple} {...props} />;
}

export function GithubMark(props: MarkProps) {
  return <Mark icon={siGithub} {...props} />;
}

export function LinuxMark(props: MarkProps) {
  return <Mark icon={siLinux} {...props} />;
}
