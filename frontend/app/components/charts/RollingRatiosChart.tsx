"use client";

import { useEffect, useRef } from "react";
import * as d3 from "d3";
import {
  drawYGrid,
  ensure,
  parseDates,
  ratioTick,
  setPath,
  shapeKey,
  useMeasure,
  xTickCount,
  type ChartHover,
  type Margin,
  type Sel,
} from "@/app/lib/chart/base";

const M: Margin = { top: 10, right: 10, bottom: 22, left: 40 };
const PLOT_H = 200;
export const ROLLING_H = PLOT_H + M.top + M.bottom;

const ACCENT = "var(--accent)";
const SORTINO = "#34d399";

export interface RollingRatiosChartProps {
  /** ISO dates, aligned to both series. */
  dates: string[];
  /** Null until a full rolling window is available. */
  sharpe: (number | null)[];
  sortino: (number | null)[];
  hoverIndex: number | null;
  onHover: (h: ChartHover | null) => void;
}

export default function RollingRatiosChart({
  dates,
  sharpe,
  sortino,
  hoverIndex,
  onHover,
}: RollingRatiosChartProps) {
  const { ref: boxRef, width } = useMeasure<HTMLDivElement>();
  const svgRef = useRef<SVGSVGElement | null>(null);
  const onHoverRef = useRef(onHover);
  useEffect(() => {
    onHoverRef.current = onHover;
  }, [onHover]);
  // Previous draw's shape, so the effect can tell a data change (safe to
  // tween) from a resize or a series-length change (redraw hard).
  const prevShape = useRef<string | null>(null);

  const innerW = Math.max(0, width - M.left - M.right);
  const n = dates.length;
  const scales = useRef<{
    x: d3.ScaleTime<number, number>;
    dates: Date[];
  } | null>(null);

  useEffect(() => {
    if (!svgRef.current || innerW <= 0 || n < 2) return;
    const t = parseDates(dates);
    const present = [...sharpe, ...sortino].filter(
      (v): v is number => v != null,
    );
    if (present.length < 2) return;

    const key = shapeKey(n, width);
    const animate = prevShape.current === key;
    prevShape.current = key;

    let min = Math.min(...present, 0);
    let max = Math.max(...present, 0);
    if (min === max) {
      min -= 1;
      max += 1;
    }

    const x = d3.scaleUtc().domain([t[0], t[n - 1]]).range([0, innerW]);
    const y = d3.scaleLinear().domain([min, max]).nice().range([PLOT_H, 0]);
    scales.current = { x, dates: t };

    const svg = d3.select(svgRef.current) as Sel;
    const g = ensure(svg, "g", "root").attr(
      "transform",
      `translate(${M.left},${M.top})`,
    );

    drawYGrid(g, y, innerW, 5);

    // Zero line, drawn only when the domain actually straddles it.
    ensure(g, "g", "zero")
      .selectAll("line")
      .data(min < 0 && max > 0 ? [0] : [])
      .join("line")
      .attr("x1", 0)
      .attr("x2", innerW)
      .attr("y1", y(0))
      .attr("y2", y(0))
      .attr("stroke", "rgba(255,255,255,.14)")
      .attr("stroke-width", 1)
      .attr("stroke-dasharray", "3 4");

    const line = d3
      .line<number | null>()
      .defined((v) => v != null)
      .x((_, i) => x(t[i]))
      .y((v) => y(v as number));

    const marks = ensure(g, "g", "marks");
    setPath(
      ensure(marks, "path", "sortino")
        .attr("fill", "none")
        .attr("stroke", SORTINO)
        .attr("stroke-width", 1.6),
      line(sortino) ?? "",
      animate,
    );
    setPath(
      ensure(marks, "path", "sharpe")
        .attr("fill", "none")
        .attr("stroke", ACCENT)
        .attr("stroke-width", 2),
      line(sharpe) ?? "",
      animate,
    );

    ensure(g, "g", "x-axis d3-axis")
      .attr("transform", `translate(0,${PLOT_H})`)
      .call(
        d3
          .axisBottom(x)
          .ticks(xTickCount(innerW))
          .tickSizeOuter(0) as unknown as (sel: Sel) => void,
      );
    ensure(g, "g", "y-axis d3-axis").call(
      d3
        .axisLeft(y)
        .ticks(5)
        .tickFormat(ratioTick)
        .tickSizeOuter(0) as unknown as (sel: Sel) => void,
    );

    ensure(g, "g", "cross").style("pointer-events", "none");

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
  }, [dates, sharpe, sortino, width, innerW, n]);

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
      .attr("stroke", ACCENT)
      .attr("stroke-width", 1)
      .attr("opacity", 0.5);
  }, [hoverIndex, width, dates]);

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
        height={ROLLING_H}
        style={{ display: "block" }}
      />
    </div>
  );
}
