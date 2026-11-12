/**
 * Where a plugin's component meets the shell: its host in context, a spinner
 * while its lazy chunk loads, and an error boundary so a plugin that throws
 * takes down its own block and nothing else.
 */
import { PluginHostProvider, type Icon, type PluginHost } from "@droppen/plugin-sdk";
import { PuzzlePieceIcon } from "@phosphor-icons/react";
import { Component, Suspense, type ErrorInfo, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Spinner } from "@/components/ui/spinner";

interface CatchProps {
  pluginId: string;
  children: ReactNode;
  fallback: (error: Error, retry: () => void) => ReactNode;
}

class Catch extends Component<CatchProps, { error: Error | null }> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`plugin ${this.props.pluginId} crashed:`, error, info.componentStack);
  }
  render() {
    if (this.state.error) return this.props.fallback(this.state.error, () => this.setState({ error: null }));
    return this.props.children;
  }
}

function Crashed({ name, error, retry }: { name: string; error: Error; retry: () => void }) {
  const { t } = useTranslation();
  return (
    <div role="alert" data-signal="failed" className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 p-4 text-center">
      <span className="text-xs text-danger">{t("the {{name}} plugin stopped working", { name })}</span>
      <span className="max-w-md font-mono text-2xs break-words text-faint">{error.message}</span>
      <button
        type="button"
        onClick={retry}
        className="flex h-7 items-center border border-border-strong px-3 text-xs text-fg hover:bg-hover"
      >
        {t("retry")}
      </button>
    </div>
  );
}

export function PluginBoundary({ host, name, children }: { host: PluginHost; name: string; children: ReactNode }) {
  return (
    <PluginHostProvider value={host}>
      <Catch pluginId={host.pluginId} fallback={(error, retry) => <Crashed name={name} error={error} retry={retry} />}>
        <Suspense fallback={<Spinner className="flex-1" />}>{children}</Suspense>
      </Catch>
    </PluginHostProvider>
  );
}

/**
 * A plugin's icon, in the shell's rail or pane picker. Rendered inside its own
 * boundary: an icon that throws is drawn as a generic one instead of taking
 * the navigation down with it.
 */
export function PluginIcon({ host, icon: Glyph, className }: { host: PluginHost; icon: Icon; className?: string }) {
  return (
    <PluginHostProvider value={host}>
      <Catch pluginId={host.pluginId} fallback={() => <PuzzlePieceIcon className={className} aria-hidden />}>
        <Glyph className={className} aria-hidden />
      </Catch>
    </PluginHostProvider>
  );
}
