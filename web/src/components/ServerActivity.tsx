import { useT } from "../i18n";
import type { ProgressState, Status } from "../types";

/**
 * The single owner of lifecycle/provisioning activity on the server page.
 * Console is reserved for process output; this strip is where download work
 * is represented, including after a reconnect from the current snapshot.
 */
export function ServerActivity({
  status,
  progress,
}: {
  status: Status;
  progress: ProgressState | null;
}) {
  const t = useT();

  if (status === "offline" || status === "online" || status === "crashed") return null;

  const stage =
    status === "preparing"
      ? progress?.stage ?? t("status.preparing")
      : status === "starting"
        ? t("server.startingActivity")
        : t("server.stoppingActivity");
  const fraction =
    status === "preparing" && progress?.fraction !== null && progress?.fraction !== undefined
      ? Math.max(0, Math.min(1, progress.fraction))
      : null;

  return (
    <section
      class="rounded-xl border border-sky-500/40 bg-sky-500/10 px-4 py-3"
      data-testid="server-activity"
      aria-live="polite"
    >
      <div class="flex items-center justify-between gap-3 text-sm text-sky-100">
        <span class="truncate font-medium">{stage}</span>
        {fraction !== null && (
          <span class="shrink-0 tabular-nums">{Math.round(fraction * 100)}%</span>
        )}
      </div>
      {fraction !== null && (
        <div class="mt-2 h-1.5 overflow-hidden rounded-full bg-sky-900">
          <div
            class="h-full bg-sky-400 transition-[width] duration-300"
            style={{ width: `${fraction * 100}%` }}
          />
        </div>
      )}
    </section>
  );
}
