/**
 * Canvas render layer for the pill's live waveform (issue #126, M2 PR 2.3).
 * Thin and intentionally untested (view-only render code, mirroring
 * AGENTS.md's OS-integration-glue exemption: "keep all logic out of it so
 * the logic stays testable") -- every decision about which bars to draw
 * lives in `src/lib/waveform.ts`'s `barsFromLevels`, which is fully unit
 * tested; this component only paints the numbers it's given onto a plain
 * 2D canvas context (no charting library -- the canvas element and its 2D
 * context are already part of every browser/webview, so this adds no new
 * dependency).
 */
import { useEffect, useRef } from "react";

const WIDTH = 96;
const HEIGHT = 20;
const BAR_GAP = 2;
const MIN_BAR_HEIGHT = 2;

export function PillWaveform({ bars }: { bars: readonly number[] }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!ctx) return;

    ctx.clearRect(0, 0, WIDTH, HEIGHT);
    if (bars.length === 0) return;

    const barWidth = WIDTH / bars.length - BAR_GAP;
    ctx.fillStyle = "rgb(228 228 231)"; // neutral-200 -- readable on the pill's dark bg
    bars.forEach((level, index) => {
      const barHeight = Math.max(MIN_BAR_HEIGHT, level * HEIGHT);
      const x = index * (barWidth + BAR_GAP);
      const y = (HEIGHT - barHeight) / 2;
      ctx.fillRect(x, y, barWidth, barHeight);
    });
  }, [bars]);

  return (
    <canvas
      ref={canvasRef}
      width={WIDTH}
      height={HEIGHT}
      aria-hidden
      className="shrink-0"
      data-testid="pill-waveform"
    />
  );
}
