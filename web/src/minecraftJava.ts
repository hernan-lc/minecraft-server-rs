/**
 * Minecraft-version-to-Java-version mapping for the create-server form.
 *
 * Current Minecraft 26.x releases require Java 25, while the 1.20/1.21 era
 * runs on Java 21. The create-server form auto-selects this default whenever
 * the Minecraft version changes; the operator can still override it.
 *
 * The demo automation (`web/demo/first-run.ts`) verifies this selection
 * instead of silently overriding it, so a regression here fails the demo.
 */
export function recommendedJavaForVersion(version: string): number {
  const trimmed = version.trim();
  if (!trimmed) return 21;
  // Leading numeric component: "26.2" -> 26, "26.3-snapshot-9" -> 26,
  // "1.21.8" -> 1. Anything unparseable keeps the previous default.
  const first = Number.parseInt(trimmed.split(/[.\-+_]/, 1)[0] ?? "", 10);
  if (Number.isFinite(first) && first >= 26) return 25;
  return 21;
}
