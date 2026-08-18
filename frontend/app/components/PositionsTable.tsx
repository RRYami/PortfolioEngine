"use client";

import { Fragment, useMemo, useState } from "react";
import type { CSSProperties } from "react";
import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  getExpandedRowModel,
  getFilteredRowModel,
  getPaginationRowModel,
  getSortedRowModel,
  useReactTable,
  type SortingState,
} from "@tanstack/react-table";
import type { PositionDetail } from "@/app/lib/positionsTypes";
import { ccySym, comp, MINUS, nf, qty } from "@/app/lib/format";

const GAIN = "var(--gain)";
const LOSS = "var(--loss)";

/** Native-currency money, e.g. `$64.50`. */
function money(n: number, ccy: string, d = 2): string {
  return ccySym(ccy) + nf(n, d);
}

/** Signed compact money, e.g. `+€1.2k`. */
function signedComp(n: number, ccy: string): string {
  return (n >= 0 ? "+" : MINUS) + comp(Math.abs(n), ccy);
}

/** Muted second line under a base-currency figure, showing the native one. */
const subLine: CSSProperties = { fontSize: 10, color: "#6b7280", marginTop: 1 };

const th: CSSProperties = {
  padding: "9px 12px",
  textAlign: "right",
  whiteSpace: "nowrap",
  userSelect: "none",
};
const td: CSSProperties = {
  padding: "10px 12px",
  textAlign: "right",
  fontSize: 12,
};

const col = createColumnHelper<PositionDetail>();

export default function PositionsTable({
  positions,
  baseCcy,
  onSell,
}: {
  positions: PositionDetail[];
  /** Portfolio base currency — every converted figure is rendered in it. */
  baseCcy: string;
  onSell: (p: PositionDetail) => void;
}) {
  const [sorting, setSorting] = useState<SortingState>([
    { id: "marketValue", desc: true },
  ]);
  const [globalFilter, setGlobalFilter] = useState("");

  const columns = useMemo(
    () => [
      col.display({
        id: "expander",
        header: () => null,
        cell: ({ row }) => (
          <button
            onClick={row.getToggleExpandedHandler()}
            aria-label={row.getIsExpanded() ? "Collapse lots" : "Expand lots"}
            style={{
              cursor: "pointer",
              background: "transparent",
              border: 0,
              color: row.getIsExpanded() ? "var(--accent)" : "#6b7280",
              fontSize: 11,
              width: 18,
              transition: "transform .12s",
              transform: row.getIsExpanded() ? "rotate(90deg)" : "none",
            }}
          >
            ▶
          </button>
        ),
      }),
      col.accessor("ticker", {
        header: "Asset",
        cell: ({ row }) => {
          const p = row.original;
          return (
            <div style={{ textAlign: "left" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                <span style={{ fontWeight: 700, fontSize: 12.5 }}>{p.ticker}</span>
                <span
                  className="mono"
                  style={{
                    fontSize: 8.5,
                    padding: "1px 4px",
                    borderRadius: 4,
                    background: "rgba(255,255,255,.06)",
                    color: "#7b8394",
                  }}
                >
                  {p.ccy}
                </span>
                <span style={{ fontSize: 9.5, color: "#4b5263" }}>
                  {p.lots.length} lot{p.lots.length === 1 ? "" : "s"}
                </span>
              </div>
              <div
                style={{
                  fontSize: 10.5,
                  color: "#6b7280",
                  marginTop: 2,
                  maxWidth: 220,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                {p.name}
              </div>
            </div>
          );
        },
        sortingFn: "text",
      }),
      col.accessor("quantity", {
        header: "Quantity",
        cell: (c) => <span className="mono">{qty(c.getValue())}</span>,
      }),
      col.accessor("avgCost", {
        header: "Avg Cost",
        cell: (c) => (
          <span className="mono" style={{ color: "#9aa1b2" }}>
            {money(c.getValue(), c.row.original.ccy)}
          </span>
        ),
      }),
      col.accessor("last", {
        header: "Last",
        cell: (c) => (
          <span className="mono" style={{ color: "#c5cad6" }}>
            {money(c.getValue(), c.row.original.ccy)}
          </span>
        ),
      }),
      col.accessor("marketValue", {
        header: "Mkt Value",
        cell: (c) => {
          const p = c.row.original;
          return (
            <div>
              <div className="mono" style={{ color: "#e8eaf0" }}>
                {comp(c.getValue(), baseCcy)}
              </div>
              {p.ccy !== baseCcy && (
                <div className="mono" style={subLine}>
                  {money(p.marketValueNative, p.ccy, 0)}
                </div>
              )}
            </div>
          );
        },
      }),
      col.accessor("weightPct", {
        header: "Weight",
        cell: (c) => {
          const w = c.getValue();
          return (
            <div style={{ display: "flex", alignItems: "center", justifyContent: "flex-end", gap: 8 }}>
              <div
                style={{
                  width: 54,
                  height: 4,
                  borderRadius: 2,
                  background: "rgba(255,255,255,.07)",
                }}
              >
                <div
                  style={{
                    height: "100%",
                    borderRadius: 2,
                    background: "var(--accent)",
                    width: `${Math.min(100, w)}%`,
                  }}
                />
              </div>
              <span className="mono" style={{ color: "#9aa1b2", minWidth: 38 }}>
                {w.toFixed(1)}%
              </span>
            </div>
          );
        },
      }),
      col.display({
        id: "actions",
        header: "",
        cell: (c) => (
          <button
            onClick={(e) => {
              e.stopPropagation(); // the row itself toggles the lot drill-in
              onSell(c.row.original);
            }}
            style={{
              padding: "3px 9px",
              borderRadius: 6,
              background: "transparent",
              border: "1px solid rgba(255,255,255,.12)",
              color: "#9aa1b2",
              fontSize: 10.5,
              fontWeight: 600,
              cursor: "pointer",
              whiteSpace: "nowrap",
            }}
          >
            Sell
          </button>
        ),
      }),
      col.accessor("unrealizedPnl", {
        header: "Unrl P&L",
        cell: (c) => {
          const p = c.row.original;
          const color = c.getValue() >= 0 ? GAIN : LOSS;
          return (
            <div>
              <div className="mono" style={{ color }}>
                {signedComp(c.getValue(), baseCcy)}
              </div>
              <div className="mono" style={{ color, fontSize: 10, opacity: 0.8 }}>
                {(p.unrealizedPnlPct >= 0 ? "+" : MINUS) +
                  Math.abs(p.unrealizedPnlPct).toFixed(1) +
                  "%"}
              </div>
              {p.ccy !== baseCcy && (
                <div className="mono" style={subLine}>
                  {signedComp(p.unrealizedPnlNative, p.ccy)}
                </div>
              )}
            </div>
          );
        },
      }),
    ],
    [baseCcy, onSell],
  );

  const table = useReactTable({
    data: positions,
    columns,
    state: { sorting, globalFilter },
    onSortingChange: setSorting,
    onGlobalFilterChange: setGlobalFilter,
    globalFilterFn: "includesString",
    getRowCanExpand: () => true,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    getExpandedRowModel: getExpandedRowModel(),
    getPaginationRowModel: getPaginationRowModel(),
    initialState: { pagination: { pageSize: 10 } },
  });

  const colCount = table.getAllLeafColumns().length;

  return (
    <div className="pe-panel" style={{ padding: "16px 18px" }}>
      {/* header: title + search */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          marginBottom: 14,
          gap: 12,
        }}
      >
        <div style={{ fontWeight: 700, fontSize: 13.5 }}>
          Positions{" "}
          <span className="pe-lbl" style={{ marginLeft: 6 }}>
            {table.getFilteredRowModel().rows.length} of {positions.length}
          </span>
        </div>
        <input
          value={globalFilter}
          onChange={(e) => setGlobalFilter(e.target.value)}
          placeholder="Filter ticker or name…"
          style={{
            padding: "6px 11px",
            borderRadius: 8,
            background: "#0b0d12",
            border: "1px solid rgba(255,255,255,.09)",
            color: "#e8eaf0",
            fontSize: 12,
            width: 210,
            outline: "none",
          }}
        />
      </div>

      <div style={{ overflowX: "auto" }}>
        <table
          style={{
            width: "100%",
            borderCollapse: "collapse",
            fontSize: 12,
          }}
        >
          <thead>
            {table.getHeaderGroups().map((hg) => (
              <tr
                key={hg.id}
                style={{ borderBottom: "1px solid rgba(255,255,255,.08)" }}
              >
                {hg.headers.map((header, idx) => {
                  const sorted = header.column.getIsSorted();
                  const first = idx === 1; // "Asset" column left-aligns
                  return (
                    <th
                      key={header.id}
                      onClick={header.column.getToggleSortingHandler()}
                      className="pe-lbl"
                      style={{
                        ...th,
                        textAlign: idx <= 1 ? "left" : "right",
                        cursor: header.column.getCanSort() ? "pointer" : "default",
                        paddingLeft: first ? 4 : 12,
                      }}
                    >
                      {flexRender(
                        header.column.columnDef.header,
                        header.getContext(),
                      )}
                      {sorted === "asc" && " ▲"}
                      {sorted === "desc" && " ▼"}
                    </th>
                  );
                })}
              </tr>
            ))}
          </thead>
          <tbody>
            {table.getRowModel().rows.map((row) => (
              <Fragment key={row.id}>
                <tr
                  className="pe-row"
                  style={{ borderBottom: "1px solid rgba(255,255,255,.04)" }}
                >
                  {row.getVisibleCells().map((cell, idx) => (
                    <td
                      key={cell.id}
                      style={{
                        ...td,
                        textAlign: idx <= 1 ? "left" : "right",
                        verticalAlign: "middle",
                        paddingLeft: idx === 0 ? 4 : 12,
                      }}
                    >
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </td>
                  ))}
                </tr>
                {row.getIsExpanded() && (
                  <tr>
                    <td colSpan={colCount} style={{ padding: 0 }}>
                      <LotsPanel position={row.original} baseCcy={baseCcy} />
                    </td>
                  </tr>
                )}
              </Fragment>
            ))}
            {table.getRowModel().rows.length === 0 && (
              <tr>
                <td
                  colSpan={colCount}
                  style={{ padding: 28, textAlign: "center", color: "#6b7280" }}
                >
                  No positions match the filter.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {/* pagination */}
      {table.getPageCount() > 1 && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "flex-end",
            gap: 10,
            marginTop: 14,
          }}
        >
          <span className="pe-lbl">
            Page {table.getState().pagination.pageIndex + 1} of{" "}
            {table.getPageCount()}
          </span>
          <PagerButton
            disabled={!table.getCanPreviousPage()}
            onClick={() => table.previousPage()}
          >
            ‹ Prev
          </PagerButton>
          <PagerButton
            disabled={!table.getCanNextPage()}
            onClick={() => table.nextPage()}
          >
            Next ›
          </PagerButton>
        </div>
      )}
    </div>
  );
}

function PagerButton({
  children,
  disabled,
  onClick,
}: {
  children: React.ReactNode;
  disabled: boolean;
  onClick: () => void;
}) {
  return (
    <button
      disabled={disabled}
      onClick={onClick}
      style={{
        padding: "5px 11px",
        borderRadius: 8,
        background: "#14161d",
        border: "1px solid rgba(255,255,255,.08)",
        color: disabled ? "#4b5263" : "#c5cad6",
        fontSize: 11.5,
        cursor: disabled ? "default" : "pointer",
      }}
    >
      {children}
    </button>
  );
}

/** Drill-in: the individual tax lots behind a position. */
function LotsPanel({
  position,
  baseCcy,
}: {
  position: PositionDetail;
  baseCcy: string;
}) {
  const sym = ccySym(position.ccy);
  // Only worth showing the conversion when there is one to show.
  const converted = position.ccy !== baseCcy;
  return (
    <div
      style={{
        background: "rgba(99,102,241,.05)",
        borderLeft: "2px solid var(--accent)",
        padding: "10px 14px 12px 34px",
      }}
    >
      <div className="pe-lbl" style={{ marginBottom: 8 }}>
        Tax lots · {position.name}
      </div>
      <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 11.5 }}>
        <thead>
          <tr className="pe-lbl">
            <th style={{ textAlign: "left", padding: "4px 8px" }}>Acquired</th>
            <th style={{ textAlign: "right", padding: "4px 8px" }}>Quantity</th>
            <th style={{ textAlign: "right", padding: "4px 8px" }}>Price</th>
            <th style={{ textAlign: "right", padding: "4px 8px" }}>Cost</th>
            {converted && (
              <>
                <th style={{ textAlign: "right", padding: "4px 8px" }}>
                  Trade-date FX
                </th>
                <th style={{ textAlign: "right", padding: "4px 8px" }}>
                  Cost ({baseCcy})
                </th>
              </>
            )}
          </tr>
        </thead>
        <tbody>
          {position.lots.map((lot, i) => (
            <tr key={i} style={{ borderTop: "1px solid rgba(255,255,255,.05)" }}>
              <td className="mono" style={{ padding: "5px 8px", color: "#c5cad6" }}>
                {lot.date}
              </td>
              <td
                className="mono"
                style={{ padding: "5px 8px", textAlign: "right", color: "#c5cad6" }}
              >
                {qty(lot.quantity)}
              </td>
              <td
                className="mono"
                style={{ padding: "5px 8px", textAlign: "right", color: "#c5cad6" }}
              >
                {sym + nf(lot.price, 2)}
              </td>
              <td
                className="mono"
                style={{ padding: "5px 8px", textAlign: "right", color: "#e8eaf0" }}
              >
                {sym + nf(lot.cost, 2)}
              </td>
              {converted && (
                <>
                  <td
                    className="mono"
                    style={{ padding: "5px 8px", textAlign: "right", color: "#9aa1b2" }}
                  >
                    {nf(lot.fxRate, 4)}
                  </td>
                  <td
                    className="mono"
                    style={{ padding: "5px 8px", textAlign: "right", color: "#e8eaf0" }}
                  >
                    {ccySym(baseCcy) + nf(lot.costBase, 2)}
                  </td>
                </>
              )}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
