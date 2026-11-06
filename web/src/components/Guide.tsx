import { createPortal } from "preact/compat";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { useT } from "../i18n";
import * as Icon from "./icons";
import { Button } from "./ui";

export type GuidePlacement = "auto" | "top" | "bottom" | "left" | "right";

export interface GuideStep {
  title: string;
  body: string;
  /**
   * CSS selector of the element to spotlight, usually `[data-tour="…"]`.
   * Without one (or when nothing visible matches) the step is shown as a
   * centred card over a dimmed page.
   */
  target?: string;
  /** Preferred side for the popover; flips automatically when it does not fit. */
  placement?: GuidePlacement;
  /** Runs before the target is looked up, e.g. to switch to the right tab. */
  enter?: () => void;
  /** Skip the step entirely when its target never appears (e.g. no servers yet). */
  optional?: boolean;
  /** Let the reader click the highlighted element while the tour is open. */
  interactive?: boolean;
}

interface Rect {
  top: number;
  left: number;
  width: number;
  height: number;
}

type Side = Exclude<GuidePlacement, "auto">;

interface PopoverLayout {
  top: number;
  left: number;
  side: Side | "center";
  /** Offset of the arrow along the popover edge, in px. */
  arrow: number;
}

const STORAGE_PREFIX = "mcpanel:guide:v2:";
const PADDING = 8;
const GAP = 14;
const MARGIN = 12;
const WAIT_FOR_TARGET_MS = 1500;
const ANIMATION_MS = 220;

function prefersReducedMotion(): boolean {
  try {
    return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
  } catch {
    return false;
  }
}

/** First matching element that is actually laid out (responsive layouts often duplicate controls). */
function findTarget(selector: string): HTMLElement | null {
  let matches: HTMLElement[];
  try {
    matches = Array.from(document.querySelectorAll<HTMLElement>(selector));
  } catch {
    return null;
  }
  return (
    matches.find((element) => {
      const rect = element.getBoundingClientRect();
      return rect.width > 0 || rect.height > 0;
    }) ?? null
  );
}

function measure(element: HTMLElement): Rect {
  const rect = element.getBoundingClientRect();
  return {
    top: rect.top - PADDING,
    left: rect.left - PADDING,
    width: rect.width + PADDING * 2,
    height: rect.height + PADDING * 2,
  };
}

function isFullyVisible(element: HTMLElement): boolean {
  const rect = element.getBoundingClientRect();
  return (
    rect.top >= MARGIN &&
    rect.left >= 0 &&
    rect.bottom <= window.innerHeight - MARGIN &&
    rect.right <= window.innerWidth
  );
}

const clamp = (value: number, min: number, max: number) =>
  Math.max(min, Math.min(value, Math.max(min, max)));

/** Places the popover next to the spotlight, flipping sides like driver.js / Popper. */
export function computePopoverLayout(
  target: Rect | null,
  popover: { width: number; height: number },
  viewport: { width: number; height: number },
  preferred: GuidePlacement = "auto",
): PopoverLayout {
  const centred = (): PopoverLayout => ({
    top: Math.max(MARGIN, (viewport.height - popover.height) / 2),
    left: Math.max(MARGIN, (viewport.width - popover.width) / 2),
    side: "center",
    arrow: 0,
  });
  if (!target) return centred();

  const space: Record<Side, number> = {
    top: target.top - MARGIN,
    bottom: viewport.height - (target.top + target.height) - MARGIN,
    left: target.left - MARGIN,
    right: viewport.width - (target.left + target.width) - MARGIN,
  };
  const fits = (side: Side) =>
    side === "top" || side === "bottom"
      ? space[side] >= popover.height + GAP
      : space[side] >= popover.width + GAP;

  const order: Side[] =
    preferred === "auto"
      ? ["bottom", "top", "right", "left"]
      : [preferred, ...(["bottom", "top", "right", "left"] as Side[]).filter((s) => s !== preferred)];
  const side = order.find(fits);

  const centreX = target.left + target.width / 2;
  const centreY = target.top + target.height / 2;

  if (!side) {
    // The target fills the screen (e.g. a large panel): float the popover over
    // the emptier half instead of pushing it off-screen.
    const top =
      space.bottom >= space.top
        ? viewport.height - popover.height - MARGIN
        : MARGIN;
    return {
      top: Math.max(MARGIN, top),
      left: clamp(centreX - popover.width / 2, MARGIN, viewport.width - popover.width - MARGIN),
      side: "center",
      arrow: 0,
    };
  }

  if (side === "top" || side === "bottom") {
    const left = clamp(centreX - popover.width / 2, MARGIN, viewport.width - popover.width - MARGIN);
    return {
      top: side === "bottom" ? target.top + target.height + GAP : target.top - popover.height - GAP,
      left,
      side,
      arrow: clamp(centreX - left, 16, popover.width - 16),
    };
  }
  const top = clamp(centreY - popover.height / 2, MARGIN, viewport.height - popover.height - MARGIN);
  return {
    top,
    left: side === "right" ? target.left + target.width + GAP : target.left - popover.width - GAP,
    side,
    arrow: clamp(centreY - top, 16, popover.height - 16),
  };
}

function readSeen(storageKey?: string): boolean {
  if (!storageKey) return true;
  try {
    return localStorage.getItem(STORAGE_PREFIX + storageKey) !== null;
  } catch {
    return true;
  }
}

function markSeen(storageKey?: string) {
  if (!storageKey) return;
  try {
    localStorage.setItem(STORAGE_PREFIX + storageKey, "seen");
  } catch {
    /* storage is optional */
  }
}

/**
 * A driver.js-style product tour: dims the page, cuts a spotlight around the
 * element being explained and anchors a popover to it.
 *
 * Bundled (no CDN), translated through `useT`, keyboard-driven
 * (←/→, Enter, Esc) and focus-trapped. Steps may switch UI state through
 * `enter`; the tour waits for the target to render before moving the light.
 */
export function Guide({
  steps,
  storageKey,
  label,
}: {
  steps: GuideStep[];
  /** When set, the tour opens automatically once per key and remembers dismissal. */
  storageKey?: string;
  /** Text of the launcher button; defaults to the translated "Help" label. */
  label?: string;
}) {
  const t = useT();
  const [step, setStep] = useState<number | null>(() => (readSeen(storageKey) ? null : 0));

  return (
    <>
      <Button
        variant="ghost"
        icon={<Icon.Info size={15} />}
        data-tour="guide-help"
        aria-label={label ?? t("guide.help")}
        title={label ?? t("guide.help")}
        class="shrink-0 max-sm:!px-2.5"
        onClick={() => setStep(0)}
      >
        <span class="hidden sm:inline">{label ?? t("guide.help")}</span>
      </Button>
      {step !== null && steps.length > 0 && (
        <Tour
          steps={steps}
          index={step}
          onIndex={setStep}
          onClose={() => {
            setStep(null);
            markSeen(storageKey);
          }}
        />
      )}
    </>
  );
}

function Tour({
  steps,
  index,
  onIndex,
  onClose,
}: {
  steps: GuideStep[];
  index: number;
  onIndex: (index: number) => void;
  onClose: () => void;
}) {
  const t = useT();
  const current = steps[Math.min(index, steps.length - 1)];
  const [target, setTarget] = useState<HTMLElement | null>(null);
  const [resolved, setResolved] = useState(false);
  const [rect, setRect] = useState<Rect | null>(null);
  const [layout, setLayout] = useState<PopoverLayout | null>(null);
  const [viewport, setViewport] = useState(() => ({
    width: window.innerWidth,
    height: window.innerHeight,
  }));
  const popover = useRef<HTMLDivElement | null>(null);
  const direction = useRef<1 | -1>(1);
  const shownRect = useRef<Rect | null>(null);
  const animation = useRef<number | null>(null);
  const opener = useRef<HTMLElement | null>(
    document.activeElement instanceof HTMLElement ? document.activeElement : null,
  );
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  const go = useCallback(
    (next: number) => {
      direction.current = next < index ? -1 : 1;
      if (next < 0) return;
      if (next >= steps.length) onCloseRef.current();
      else onIndex(next);
    },
    [index, steps.length, onIndex],
  );

  // Restore focus to whatever opened the tour.
  useEffect(
    () => () => {
      const element = opener.current;
      if (element && document.contains(element)) {
        try {
          element.focus();
        } catch {
          /* closing must still win */
        }
      }
    },
    [],
  );

  // Enter the step, then wait for its target to exist (tabs render lazily).
  useEffect(() => {
    setResolved(false);
    setTarget(null);
    current.enter?.();
    if (!current.target) {
      setResolved(true);
      return;
    }
    const selector = current.target;
    const started = performance.now();
    let frame = 0;
    let cancelled = false;
    const poll = () => {
      if (cancelled) return;
      const found = findTarget(selector);
      if (found) {
        if (!isFullyVisible(found)) {
          found.scrollIntoView({
            block: "center",
            inline: "nearest",
            behavior: prefersReducedMotion() ? "auto" : "smooth",
          });
        }
        setTarget(found);
        setResolved(true);
        return;
      }
      if (performance.now() - started > WAIT_FOR_TARGET_MS) {
        if (current.optional) {
          // Keep moving in the direction the reader was going.
          const next = index + direction.current;
          if (next < 0) onIndex(index + 1 < steps.length ? index + 1 : index);
          else go(next);
          return;
        }
        setResolved(true);
        return;
      }
      frame = requestAnimationFrame(poll);
    };
    poll();
    return () => {
      cancelled = true;
      cancelAnimationFrame(frame);
    };
  }, [index]);

  // Track the target through scrolling, resizing and layout changes.
  useEffect(() => {
    if (!resolved) return;
    const update = (animate: boolean) => {
      setViewport({ width: window.innerWidth, height: window.innerHeight });
      const next = target && document.contains(target) ? measure(target) : null;
      const from = shownRect.current;
      if (animation.current !== null) cancelAnimationFrame(animation.current);
      if (!animate || !next || !from || prefersReducedMotion()) {
        shownRect.current = next;
        setRect(next);
        return;
      }
      const start = performance.now();
      const tick = (now: number) => {
        const p = Math.min(1, (now - start) / ANIMATION_MS);
        const e = 1 - Math.pow(1 - p, 3);
        const value = {
          top: from.top + (next.top - from.top) * e,
          left: from.left + (next.left - from.left) * e,
          width: from.width + (next.width - from.width) * e,
          height: from.height + (next.height - from.height) * e,
        };
        shownRect.current = value;
        setRect(value);
        animation.current = p < 1 ? requestAnimationFrame(tick) : null;
      };
      animation.current = requestAnimationFrame(tick);
    };
    update(true);
    const instant = () => update(false);
    window.addEventListener("resize", instant);
    window.addEventListener("scroll", instant, true);
    const observer =
      typeof ResizeObserver !== "undefined" && target ? new ResizeObserver(instant) : null;
    if (target) observer?.observe(target);
    // Smooth scrolling has no reliable end event; re-measure while it settles.
    const settle = window.setTimeout(instant, 400);
    return () => {
      window.removeEventListener("resize", instant);
      window.removeEventListener("scroll", instant, true);
      observer?.disconnect();
      window.clearTimeout(settle);
      if (animation.current !== null) cancelAnimationFrame(animation.current);
    };
  }, [resolved, target]);

  // Position the popover once its size is known.
  useLayoutEffect(() => {
    if (!resolved || !popover.current) return;
    const box = popover.current.getBoundingClientRect();
    setLayout(
      computePopoverLayout(
        rect,
        { width: box.width || 320, height: box.height || 180 },
        viewport,
        current.placement,
      ),
    );
  }, [rect, viewport, resolved, index]);

  // Move focus to the primary action on every step.
  useEffect(() => {
    if (!resolved) return;
    popover.current
      ?.querySelector<HTMLElement>("[data-guide-next]")
      ?.focus({ preventScroll: true });
  }, [resolved, index]);

  // Keyboard navigation and focus trap.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const panel = popover.current;
      if (!panel) return;
      if (event.key === "Escape") {
        event.preventDefault();
        onCloseRef.current();
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        go(index + 1);
      } else if (event.key === "ArrowLeft") {
        event.preventDefault();
        if (index > 0) go(index - 1);
      } else if (event.key === "Tab") {
        const focusable = Array.from(
          panel.querySelectorAll<HTMLElement>("button:not([disabled])"),
        );
        if (focusable.length === 0) return;
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        const active = document.activeElement;
        if (!panel.contains(active)) {
          event.preventDefault();
          first.focus();
        } else if (event.shiftKey && active === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && active === last) {
          event.preventDefault();
          first.focus();
        }
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [index, go]);

  if (typeof document === "undefined") return null;

  const last = index === steps.length - 1;
  const { width: vw, height: vh } = viewport;
  const hole = rect
    ? {
        x: Math.max(0, rect.left),
        y: Math.max(0, rect.top),
        w: Math.max(0, Math.min(rect.width, vw - Math.max(0, rect.left))),
        h: Math.max(0, Math.min(rect.height, vh - Math.max(0, rect.top))),
      }
    : null;
  const radius = 12;
  const overlayPath = hole
    ? `M0 0H${vw}V${vh}H0Z ` +
      `M${hole.x + radius} ${hole.y}` +
      `H${hole.x + hole.w - radius}a${radius} ${radius} 0 0 1 ${radius} ${radius}` +
      `V${hole.y + hole.h - radius}a${radius} ${radius} 0 0 1 -${radius} ${radius}` +
      `H${hole.x + radius}a${radius} ${radius} 0 0 1 -${radius} -${radius}` +
      `V${hole.y + radius}a${radius} ${radius} 0 0 1 ${radius} -${radius}Z`
    : `M0 0H${vw}V${vh}H0Z`;

  const titleId = "guide-popover-title";
  const bodyId = "guide-popover-body";

  const tour = (
    <div class="fixed inset-0 z-[60]" data-testid="guide-tour">
      {/* Clicks on the dimmed area are swallowed rather than closing the tour,
          so a stray click never loses the reader's place. */}
      <svg
        class="absolute inset-0 h-full w-full"
        width={vw}
        height={vh}
        aria-hidden="true"
      >
        <path
          d={overlayPath}
          fill-rule="evenodd"
          class="fill-black/65 transition-opacity duration-200"
          style={{ pointerEvents: "auto" }}
        />
      </svg>

      {hole && (
        <div
          aria-hidden="true"
          class={`absolute rounded-xl ring-2 ring-accent/80 shadow-[0_0_0_4px_rgba(34,197,94,0.15),0_0_30px_rgba(34,197,94,0.25)] ${
            current.interactive ? "pointer-events-none" : ""
          }`}
          style={{ top: hole.y, left: hole.x, width: hole.w, height: hole.h }}
        />
      )}

      <div
        ref={popover}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={bodyId}
        class={`absolute w-[min(22rem,calc(100vw-1.5rem))] rounded-2xl border border-ink-600 bg-ink-850 shadow-2xl transition-[top,left,opacity] duration-200 ease-out ${
          resolved && layout ? "opacity-100" : "opacity-0"
        }`}
        style={{
          top: layout?.top ?? -9999,
          left: layout?.left ?? -9999,
        }}
      >
        {layout && layout.side !== "center" && (
          <span
            aria-hidden="true"
            class="absolute size-3 rotate-45 border-ink-600 bg-ink-850"
            style={arrowStyle(layout)}
          />
        )}

        <div class="relative px-5 pb-4 pt-4">
          <div class="mb-2 flex items-center justify-between gap-3">
            <p class="text-xs font-medium text-accent" aria-live="polite">
              {t("guide.step", { current: index + 1, total: steps.length })}
            </p>
            <button
              type="button"
              onClick={() => onCloseRef.current()}
              aria-label={t("guide.skip")}
              class="-mr-1.5 grid size-7 shrink-0 place-items-center rounded-lg text-fg-muted transition-colors hover:bg-ink-700 hover:text-fg focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/60"
            >
              <Icon.X size={15} />
            </button>
          </div>
          <h2 id={titleId} class="text-base font-semibold text-fg">
            {current.title}
          </h2>
          <p id={bodyId} class="mt-1.5 text-sm leading-relaxed text-fg-muted">
            {current.body}
          </p>
        </div>

        <footer class="flex items-center justify-between gap-3 border-t border-ink-700 px-5 py-3">
          <div class="flex items-center gap-1" aria-hidden="true">
            {steps.map((_, i) => (
              <span
                key={i}
                class={`h-1.5 rounded-full transition-all duration-200 ${
                  i === index ? "w-4 bg-accent" : i < index ? "w-1.5 bg-accent/50" : "w-1.5 bg-ink-600"
                }`}
              />
            ))}
          </div>
          <div class="flex items-center gap-2">
            {index > 0 && (
              <Button variant="ghost" class="!px-3 !py-1.5" onClick={() => go(index - 1)}>
                {t("guide.previous")}
              </Button>
            )}
            <Button
              data-guide-next
              variant="primary"
              class="!px-4 !py-1.5"
              onClick={() => go(index + 1)}
            >
              {last ? t("guide.done") : t("guide.next")}
            </Button>
          </div>
        </footer>
      </div>
    </div>
  );

  return createPortal(tour, document.body);
}

function arrowStyle(layout: PopoverLayout): Record<string, string | number> {
  const offset = layout.arrow - 6;
  switch (layout.side) {
    case "bottom":
      return { top: -7, left: offset, borderTopWidth: 1, borderLeftWidth: 1 };
    case "top":
      return { bottom: -7, left: offset, borderBottomWidth: 1, borderRightWidth: 1 };
    case "right":
      return { left: -7, top: offset, borderBottomWidth: 1, borderLeftWidth: 1 };
    case "left":
      return { right: -7, top: offset, borderTopWidth: 1, borderRightWidth: 1 };
    default:
      return {};
  }
}
