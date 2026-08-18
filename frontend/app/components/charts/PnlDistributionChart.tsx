"use client";

import { useEffect, useRef } from "react";
import * as d3 from "d3";
import { comp } from "@/app/lib/format";
import {
  drawYGrid,
  ensure,
  shapeKey,
  useMeasure,
  xTickCount,
  type ChartHover,
  type Margin,
  type Sel,
} from "@/app/lib/chart/base";

const M: Margin = { top: 10, right: 10, bottom: 22, left: 44 };
const PLOT_H = 208;
export const PNL_DIST_H = PLOT_H + M.top + M.bottom;

export interface PnlDistributionChartProps {
  binLow: number;
  binHigh: number;
  binCount: number;
  counts: number[];
  paths: number;
  /** P&L value of the VaR cutoff (negative). */
  varV: number;
  /** P&L value of the ES cutoff (negative). */
  esV: number;
  /** P&L value of the deep-tail colour boundary (negative). */
  deepV: number;
  hoverIndex: number | null;
  onHover: (h: ChartHover | null) => void;
}

export default function PnlDistributionChart({
  binLow,
  binHigh,
  binCount,
  counts,
  paths,
  varV,
  esV,
  deepV,
  hoverIndex,
  onHover,
}: PnlDistributionChartProps) {
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
  const scales = useRef<{
    x: d3.ScaleLinear<number, number>;
    bw: number;
    lo: number;
  } | null>(null);

  useEffect(() => {
    if (!svgRef.current || innerW <= 0 || binCount < 1) return;
    const bw = (binHigh - binLow) / binCount;

    const key = shapeKey(binCount, width);
    const animate = prevShape.current === key;
    prevShape.current = key;
    // Probability, not raw path count — the y axis now has to mean something.
    const prob = counts.map((c) => (paths > 0 ? (c / paths) * 100 : 0));

    const x = d3.scaleLinear().domain([binLow, binHigh]).range([0, innerW]);
    const y = d3
      .scaleLinear()
      .domain([0, d3.max(prob) || 1])
      .nice()
      .range([PLOT_H, 0]);
    scales.current = { x, bw, lo: binLow };

    const svg = d3.select(svgRef.current) as Sel;
    const g = ensure(svg, "g", "root").attr(
      "transform",
      `translate(${M.left},${M.top})`,
    );

    drawYGrid(g, y, innerW, 4);

    const bandW = Math.max(1, innerW / binCount - 2);
    const bars = ensure(g, "g", "bars")
      .selectAll<SVGRectElement, number>("rect")
      .data(prob)
      .join(
        (enter) =>
          enter
            .append("rect")
            .attr("rx", 1.5)
            .attr("y", y(0))
            .attr("height", 0),
        (update) => update,
        (exit) => exit.remove(),
      )
      .attr("x", (_, i) => x(binLow + i * bw) + 1)
      .attr("width", bandW)
      .attr("fill", (_, i) => {
        const center = binLow + (i + 0.5) * bw;
        if (center <= deepV) return "var(--lossDeep)";
        if (center <= varV) return "var(--loss)";
        return "var(--accent)";
      });

    // A zero-duration transition still interrupts any in-flight one, so this
    // covers both the animated and the hard-redraw case.
    bars
      .transition()
      .duration(animate ? 280 : 0)
      .attr("y", (d) => y(d))
      .attr("height", (d) => Math.max(0, y(0) - y(d)));

    // VaR / ES cutoffs.
    ensure(g, "g", "cutoffs")
      .selectAll<SVGLineElement, { v: number; c: string }>("line")
      .data([
        { v: esV, c: "var(--loss)" },
        { v: varV, c: "#fbbf24" },
      ])
      .join("line")
      .attr("x1", (d) => x(d.v))
      .attr("x2", (d) => x(d.v))
      .attr("y1", 0)
      .attr("y2", PLOT_H)
      .attr("stroke", (d) => d.c)
      .attr("stroke-width", 1.8)
      .attr("stroke-dasharray", "5 4");

    ensure(g, "g", "x-axis d3-axis")
      .attr("transform", `translate(0,${PLOT_H})`)
      .call(
        d3
          .axisBottom(x)
          .ticks(xTickCount(innerW))
          .tickFormat((v) => comp(Number(v)))
          .tickSizeOuter(0) as unknown as (sel: Sel) => void,
      );
    ensure(g, "g", "y-axis d3-axis").call(
      d3
        .axisLeft(y)
        .ticks(4)
        .tickFormat((v) => Number(v).toFixed(1) + "%")
        .tickSizeOuter(0) as unknown as (sel: Sel) => void,
    );

    ensure(g, "g", "cross").style("pointer-events", "none");

    ensure(g, "rect", "overlay")
      .attr("width", innerW)
      .attr("height", PLOT_H)
      .attr("fill", "transparent")
      .style("cursor", "crosshair")
      .on("mousemove", (event: MouseEvent) => {
        const [mx, my] = d3.pointer(event, g.node());
        const i = Math.max(
          0,
          Math.min(binCount - 1, Math.floor((x.invert(mx) - binLow) / bw)),
        );
        onHoverRef.current({
          i,
          cx: x(binLow + (i + 0.5) * bw) + M.left,
          py: my + M.top,
          w: width,
        });
      })
      .on("mouseleave", () => onHoverRef.current(null));
  }, [
    binLow,
    binHigh,
    binCount,
    counts,
    paths,
    varV,
    esV,
    deepV,
    width,
    innerW,
  ]);

  useEffect(() => {
    if (!svgRef.current || !scales.current) return;
    const g = d3.select(svgRef.current).select("g.root") as Sel;
    const s = scales.current;
    g.select("g.cross")
      .selectAll("line")
      .data(hoverIndex == null ? [] : [hoverIndex])
      .join("line")
      .attr("x1", (i) => s.x(s.lo + (i + 0.5) * s.bw))
      .attr("x2", (i) => s.x(s.lo + (i + 0.5) * s.bw))
      .attr("y1", 0)
      .attr("y2", PLOT_H)
      .attr("stroke", "var(--accent)")
      .attr("stroke-width", 1)
      .attr("opacity", 0.55);
  }, [hoverIndex, width, binCount]);

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
        height={PNL_DIST_H}
        style={{ display: "block" }}
      />
    </div>
  );
}
