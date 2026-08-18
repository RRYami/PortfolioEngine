"use client";

import { useEffect, useId, useRef } from "react";
import * as d3 from "d3";
import {
  drawYGrid,
  ensure,
  parseDates,
  pctTick,
  setPath,
  shapeKey,
  useMeasure,
  xTickCount,
  type ChartHover,
  type Margin,
  type Sel,
} from "@/app/lib/chart/base";

const M: Margin = { top: 10, right: 10, bottom: 22, left: 46 };
const PLOT_H = 150;
export const DRAWDOWN_H = PLOT_H + M.top + M.bottom;

export interface DrawdownChartProps {
  /** ISO dates, aligned to `series`. */
  dates: string[];
  /** % from running peak, ≤ 0. */
  series: number[];
  hoverIndex: number | null;
  onHover: (h: ChartHover | null) => void;
}

export default function DrawdownChart({
  dates,
  series,
  hoverIndex,
  onHover,
}: DrawdownChartProps) {
  const { ref: boxRef, width } = useMeasure<HTMLDivElement>();
  const svgRef = useRef<SVGSVGElement | null>(null);
  // Gradient ids are document-global, so scope them to this instance.
  const gradId = "dd" + useId().replace(/[^a-zA-Z0-9]/g, "");
  const onHoverRef = useRef(onHover);
  useEffect(() => {
    onHoverRef.current = onHover;
  }, [onHover]);
  // Previous draw's shape, so the effect can tell a data change (safe to
  // tween) from a resize or a series-length change (redraw hard).
  const prevShape = useRef<string | null>(null);

  const innerW = Math.max(0, width - M.left - M.right);
  const n = series.length;
  // Scales are stashed so the crosshair effect can place itself without
  // forcing a full redraw on every pointer move.
  const scales = useRef<{
    x: d3.ScaleTime<number, number>;
    dates: Date[];
  } | null>(null);

  useEffect(() => {
    if (!svgRef.current || innerW <= 0 || n < 2) return;
    const t = parseDates(dates);

    const key = shapeKey(n, width);
    const animate = prevShape.current === key;
    prevShape.current = key;

    const x = d3.scaleUtc().domain([t[0], t[n - 1]]).range([0, innerW]);
    // A book sitting at an all-time high has an all-zero series; give it a
    // nominal depth so the curve draws along the top rather than collapsing.
    const deepest = Math.min(d3.min(series) ?? 0, 0);
    const y = d3
      .scaleLinear()
      .domain([deepest === 0 ? -1 : deepest, 0])
      .nice()
      .range([PLOT_H, 0]);
    scales.current = { x, dates: t };

    const svg = d3.select(svgRef.current) as Sel;

    const defs = ensure(svg, "defs", "defs");
    const grad = ensure(defs, "linearGradient", "grad")
      .attr("id", gradId)
      .attr("x1", "0")
      .attr("y1", "0")
      .attr("x2", "0")
      .attr("y2", "1");
    grad
      .selectAll("stop")
      .data([
        { o: "0%", op: 0.42 },
        { o: "100%", op: 0.02 },
      ])
      .join("stop")
      .attr("offset", (d) => d.o)
      .attr("stop-color", "var(--loss)")
      .attr("stop-opacity", (d) => d.op);

    const g = ensure(svg, "g", "root").attr(
      "transform",
      `translate(${M.left},${M.top})`,
    );

    drawYGrid(g, y, innerW, 4);

    // Peak line: drawdown is 0 at a new high, so this is the "underwater" edge.
    ensure(g, "line", "peak")
      .attr("x1", 0)
      .attr("x2", innerW)
      .attr("y1", y(0))
      .attr("y2", y(0))
      .attr("stroke", "rgba(255,255,255,.12)")
      .attr("stroke-width", 1)
      .attr("stroke-dasharray", "3 4");

    const marks = ensure(g, "g", "marks");
    const area = d3
      .area<number>()
      .x((_, i) => x(t[i]))
      .y0(y(0))
      .y1((d) => y(d));
    const line = d3
      .line<number>()
      .x((_, i) => x(t[i]))
      .y((d) => y(d));

    setPath(
      ensure(marks, "path", "area").attr("fill", `url(#${gradId})`),
      area(series) ?? "",
      animate,
    );
    setPath(
      ensure(marks, "path", "line")
        .attr("fill", "none")
        .attr("stroke", "var(--loss)")
        .attr("stroke-width", 1.6),
      line(series) ?? "",
      animate,
    );

    ensure(g, "g", "x-axis d3-axis")
      .attr("transform", `translate(0,${PLOT_H})`)
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      .call(d3.axisBottom(x).ticks(xTickCount(innerW)).tickSizeOuter(0) as any);
    ensure(g, "g", "y-axis d3-axis")
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      .call(d3.axisLeft(y).ticks(4).tickFormat(pctTick(0)).tickSizeOuter(0) as any);

    ensure(g, "g", "cross").style("pointer-events", "none");

    // Overlay is appended last so it stays on top for hit-testing.
    const bisect = d3.bisector<Date, Date>((d) => d).center;
    ensure(g, "rect", "overlay")
      .attr("width", innerW)
      .attr("height", PLOT_H)
      .attr("fill", "transparent")
      .style("cursor", "crosshair")
      .on("mousemove", (event: MouseEvent) => {
        const [mx, my] = d3.pointer(event, g.node());
        const i = Math.max(0, Math.min(n - 1, bisect(t, x.invert(mx))));
        onHoverRef.current({
          i,
          cx: x(t[i]) + M.left,
          py: my + M.top,
          w: width,
        });
      })
      .on("mouseleave", () => onHoverRef.current(null));
  }, [dates, series, width, innerW, n, gradId]);

  // Crosshair only — kept out of the draw effect so pointer moves don't
  // re-run the joins (which would re-fire the transitions).
  useEffect(() => {
    if (!svgRef.current || !scales.current) return;
    const g = d3.select(svgRef.current).select("g.root") as Sel;
    const s = scales.current;
    g.select("g.cross")
      .selectAll("line")
      .data(hoverIndex == null || hoverIndex >= s.dates.length ? [] : [hoverIndex])
      .join("line")
      .attr("x1", (i) => s.x(s.dates[i]))
      .attr("x2", (i) => s.x(s.dates[i]))
      .attr("y1", 0)
      .attr("y2", PLOT_H)
      .attr("stroke", "var(--loss)")
      .attr("stroke-width", 1)
      .attr("opacity", 0.55);
  }, [hoverIndex, width, dates]);

  // StrictMode double-invokes effects; wiping on unmount keeps d3's nodes from
  // surviving into the remount and duplicating.
  useEffect(() => {
    const node = svgRef.current;
    return () => {
      if (node) d3.select(node).selectAll("*").remove();
      // The nodes are gone, so the next draw is a first draw, not a tween.
      prevShape.current = null;
    };
  }, []);

  return (
    <div ref={boxRef} style={{ position: "relative", width: "100%" }}>
      <svg
        ref={svgRef}
        width={width}
        height={DRAWDOWN_H}
        style={{ display: "block" }}
      />
    </div>
  );
}
