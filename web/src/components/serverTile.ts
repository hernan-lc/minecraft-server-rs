/**
 * A stable tint per server, so two similarly named servers still look
 * different. Shared by the dashboard list and the detail header so a server
 * keeps the same tile everywhere.
 */
export function tileColour(id: string | null | undefined): string {
  let hash = 0;
  for (const char of id ?? "server") hash = (hash * 31 + char.charCodeAt(0)) % 360;
  return `linear-gradient(140deg, hsl(${hash} 45% 22%), hsl(${(hash + 40) % 360} 45% 14%))`;
}
