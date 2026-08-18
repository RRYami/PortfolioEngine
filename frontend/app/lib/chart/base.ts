"use client";

// Shared plumbing for the d3-rendered charts.
//
// Ownership split: d3 owns everything *inside* each <svg> (marks, axes, grid,
// crosshair); React owns the panel chrome and the HTML tooltip overlay. The
// charts report pointer state upward as `ChartHover` — already resolved to a
// datum index and to container pixels — so the pages never re-derive geometry.

import { useEffect, useRef, useState } from "react";
import * as d3 from "d3";
import { MINUS } from "@/app/lib/format";

export interface Margin {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

/** Pointer state a chart reports so the page can place its HTML tooltip. */
export interface ChartHover {
  /** Index of the nearest datum. */
  i: number;
  /** X of that datum, in container pixels (already includes margin.left). */
  cx: number;
  /** Pointer Y, in container pixels. */
  py: number;
  /** Container width — the tooltip uses it to decide which side to flip to. */
  w: number;
}

/** Loose selection type: these charts never rely on datum typing. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type Sel = d3.Selection<any, unknown, null, undefined>;

/**
 * Measure an element's width. Replaces the old `preserveAspectRatio="none"`
 * stretch — SVG user units are now CSS pixels, so strokes and text keep their
 * true weight at any panel width.
 */
export function useMeasure<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);
  const [width, setWidth] = useState(0);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width ?? 0;
      setWidth((prev) => (Math.abs(prev - w) < 0.5 ? prev : w));
    });
    ro.observe(el);
    setWidth(el.getBoundingClientRect().width);
    return () => ro.disconnect();
  }, []);

  return { ref, width };
}

/**
 * Idempotent append. Effects re-run on every data change, so nodes have to be
 * reused rather than re-created — otherwise `.join()` sees an empty parent each
 * time and nothing can transition.
 */
export function ensure(parent: Sel, tag: string, cls: string): Sel {
  const key = cls.split(" ")[0];
  let sel = parent.select(`${tag}.${key}`);
  if (sel.empty()) sel = parent.append(tag).attr("class", cls);
  return sel as Sel;
}

/**
 * Identifies a draw's shape: point count plus pixel width. Charts compare it
 * against the previous run (inside the effect) to decide whether tweening is
 * safe — tweening a path `d` across different point counts produces garbage,
 * and tweening on every resize frame stutters, so both cases redraw hard.
 */
export function shapeKey(n: number, width: number): string {
  return `${n}:${Math.round(width)}`;
}

/** Set a path `d`, tweening only when `animate` says the shape is stable. */
export function setPath(sel: Sel, d: string, animate: boolean) {
  if (animate) sel.transition().duration(280).attr("d", d);
  else sel.interrupt().attr("d", d);
}

/** Position an HTML tooltip beside `cx`, flipping before it runs off the end. */
export function place(cx: number, py: number, w: number) {
  const left = cx > w * 0.6;
  return {
    left: (left ? cx - 12 : cx + 12) + "px",
    top: Math.max(2, py - 6) + "px",
    transform: left ? "translateX(-100%)" : "none",
  };
}

const parseIso = d3.utcParse("%Y-%m-%d");

/** ISO day strings → UTC dates, tolerating a full timestamp. */
export function parseDates(iso: string[]): Date[] {
  return iso.map((s) => parseIso(s.slice(0, 10)) ?? new Date(s));
}

/** Percent tick label using the design's Unicode minus. */
export function pctTick(decimals: number) {
  return (v: d3.NumberValue) => {
    const n = Number(v);
    return (n < 0 ? MINUS : "") + Math.abs(n).toFixed(decimals) + "%";
  };
}

/** Ratio tick label (rolling Sharpe/Sortino) using the Unicode minus. */
export const ratioTick = (v: d3.NumberValue) => {
  const n = Number(v);
  return (n < 0 ? MINUS : "") + Math.abs(n).toFixed(1);
};

/** How many x ticks a panel of this width can hold without crowding. */
export function xTickCount(innerW: number) {
  return Math.max(3, Math.min(8, Math.floor(innerW / 90)));
}

/** Horizontal gridlines behind the marks, drawn from the y scale's ticks. */
export function drawYGrid(
  g: Sel,
  y: d3.ScaleLinear<number, number>,
  innerW: number,
  ticks: number,
) {
  const grid = ensure(g, "g", "grid d3-grid");
  grid
    .selectAll("line")
    .data(y.ticks(ticks))
    .join("line")
    .attr("x1", 0)
    .attr("x2", innerW)
    .attr("y1", (d) => y(d))
    .attr("y2", (d) => y(d));
  return grid;
}
