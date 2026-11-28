import { Component, type ErrorInfo, type ReactNode } from "react";
import type { ErrorComponentProps } from "@tanstack/react-router";
import i18next from "i18next";

interface BoundaryState {
  error: Error | null;
}

function messageOf(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return i18next.t("unexpected error");
}

function ErrorFallbackView({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  return (
    <div
      role="alert"
      data-signal="failed"
      className="flex min-h-full w-full flex-1 flex-col items-center justify-center gap-3 bg-bg p-6 text-fg"
    >
      <div className="flex w-full max-w-md flex-col gap-3 border border-border bg-panel p-4">
        <div className="font-mono text-xs font-medium text-danger">{i18next.t("something went wrong")}</div>
        <div className="font-mono text-2xs break-words text-muted">{messageOf(error)}</div>
        <div className="flex gap-2">
          {onRetry && (
            <button
              type="button"
              onClick={onRetry}
              className="border border-border bg-bg px-3 py-1.5 font-mono text-2xs text-fg hover:bg-hover"
            >
              {i18next.t("retry")}
            </button>
          )}
          <button
            type="button"
            data-variant="primary"
            onClick={() => location.reload()}
            className="bg-fg px-3 py-1.5 font-mono text-2xs text-bg hover:bg-fg/85"
          >
            {i18next.t("reload")}
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * Top-level error boundary so an uncaught render failure in the sidebar,
 * CloudApp, or any view renders a recoverable fallback instead of a white page.
 */
export class AppErrorBoundary extends Component<{ children: ReactNode }, BoundaryState> {
  state: BoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): BoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("uncaught UI error:", error, info.componentStack);
  }

  render(): ReactNode {
    if (this.state.error) {
      return <ErrorFallbackView error={this.state.error} onRetry={() => this.setState({ error: null })} />;
    }
    return this.props.children;
  }
}

/** Default router error component for route loaders and view render failures. */
export function RouteErrorFallback({ error, reset }: ErrorComponentProps) {
  return <ErrorFallbackView error={error} onRetry={reset} />;
}
