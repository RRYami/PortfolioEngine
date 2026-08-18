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

const M: Margin = { top: 10, right: 10, bottom: 22, left: 48 };
const PLOT_H = 160;
export const HIST_VAR_H = PLOT_H + M.top + M.bottom;

export interface HistVarChartProps {
  /** ISO dates, aligned to both series. */
  dates: string[];
  /** Rolling 1-day VaR as a positive %. */
  v1d: number[];
  /** Rolling 20-day VaR as a positive %. */
  v20d: number[];
  hoverIndex: number | null;
  onHover: (h: ChartHover | null) => void;
}

export default function HistVarChart({
  dates,
  v1d,
  v20d,
  hoverIndex,
  onHover,
}: HistVarChartProps) {
  const { ref: boxRef, width } = useMeasure<HTMLDivElement>();
  const svgRef = useRef<SVGSVGElement | null>(null);
  // Gradient ids are document-global, so scope them to this instance.
  const gradId = "hv" + useId().replace(/[^a-zA-Z0-9]/g, "");
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

    const key = shapeKey(n, width);
    const animate = prevShape.current === key;
    prevShape.current = key;

    const x = d3.scaleUtc().domain([t[0], t[n - 1]]).range([0, innerW]);
    const y = d3
      .scaleLinear()
      .domain([0, (d3.max(v20d) ?? 0) * 1.1 || 1])
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
        { o: "0%", op: 0.26 },
        { o: "100%", op: 0.02 },
      ])
      .join("stop")
      .attr("offset", (d) => d.o)
      .attr("stop-color", "var(--accent)")
      .attr("stop-opacity", (d) => d.op);

    const g = ensure(svg, "g", "root").attr(
      "transform",
      `translate(${M.left},${M.top})`,
    );

    drawYGrid(g, y, innerW, 4);

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
      ensure(marks, "path", "area1d").attr("fill", `url(#${gradId})`),
      area(v1d) ?? "",
      animate,
    );
    setPath(
      ensure(marks, "path", "line20d")
        .attr("fill", "none")
        .attr("stroke", "var(--loss)")
        .attr("stroke-width", 1.6)
        .attr("stroke-dasharray", "5 4"),
      line(v20d) ?? "",
      animate,
    );
    setPath(
      ensure(marks, "path", "line1d")
        .attr("fill", "none")
        .attr("stroke", "var(--accent)")
        .attr("stroke-width", 2),
      line(v1d) ?? "",
      animate,
    );

    ensure(g, "g", "x-axis d3-axis")
      .attr("transform", `translate(0,${PLOT_H})`)
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      .call(d3.axisBottom(x).ticks(xTickCount(innerW)).tickSizeOuter(0) as any);
    ensure(g, "g", "y-axis d3-axis")
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      .call(d3.axisLeft(y).ticks(4).tickFormat(pctTick(1)).tickSizeOuter(0) as any);

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
  }, [dates, v1d, v20d, width, innerW, n, gradId]);

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
      .attr("stroke", "var(--accent)")
      .attr("stroke-width", 1)
      .attr("opacity", 0.55);
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
        height={HIST_VAR_H}
        style={{ display: "block" }}
      />
    </div>
  );
}
