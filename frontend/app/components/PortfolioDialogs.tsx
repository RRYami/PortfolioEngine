"use client";

import { useEffect, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ccySym, nf, qty } from "@/app/lib/format";
import type { PositionDetail } from "@/app/lib/positionsTypes";
import { Label } from "@/components/ui/label";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  CURRENCIES,
  LOT_METHODS,
  type PortfolioSummary,
} from "@/app/lib/portfolioTypes";

const accentBtn = {
  background: "var(--accent)",
  color: "#fff",
  fontWeight: 700,
};

async function postJson<T>(url: string, body: unknown): Promise<T> {
  const r = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!r.ok) {
    const raw = await r.text().catch(() => "");
    let msg = raw;
    try {
      const j = JSON.parse(raw);
      if (j && typeof j.error === "string") msg = j.error;
    } catch {
      // non-JSON body — fall back to the raw text
    }
    throw new Error(msg || `HTTP ${r.status}`);
  }
  return r.json() as Promise<T>;
}

export function NewPortfolioDialog({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  onCreated: (p: PortfolioSummary) => void;
}) {
  const [name, setName] = useState("");
  const [baseCcy, setBaseCcy] = useState("USD");
  const [lotMethod, setLotMethod] = useState("fifo");
  const [inceptionDate, setInceptionDate] = useState("2024-01-02");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    if (!name.trim()) {
      setError("Name is required");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const p = await postJson<PortfolioSummary>("/api/portfolios", {
        name: name.trim(),
        baseCcy,
        lotMethod,
        inceptionDate,
      });
      onCreated(p);
      onOpenChange(false);
      setName("");
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="dark">
        <DialogHeader>
          <DialogTitle>New portfolio</DialogTitle>
          <DialogDescription>
            Create a book; add holdings next, then run the analysis.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3 py-1">
          <Field label="Name">
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Global Macro Book"
              autoFocus
            />
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Base currency">
              <CcySelect value={baseCcy} onChange={setBaseCcy} />
            </Field>
            <Field label="Lot method">
              <Select
                value={lotMethod}
                onValueChange={(v) => v && setLotMethod(v)}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {LOT_METHODS.map((m) => (
                    <SelectItem key={m.value} value={m.value}>
                      {m.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          </div>
          <Field label="Inception date">
            <Input
              type="date"
              value={inceptionDate}
              onChange={(e) => setInceptionDate(e.target.value)}
            />
          </Field>
          {error && <p className="text-sm text-red-400">{error}</p>}
        </div>
        <DialogFooter>
          <Button onClick={submit} disabled={busy} style={accentBtn}>
            {busy ? "Creating…" : "Create portfolio"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function AddHoldingDialog({
  open,
  onOpenChange,
  portfolioId,
  defaultDate,
  onAdded,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  portfolioId: string | null;
  defaultDate?: string;
  onAdded: () => void;
}) {
  const today = new Date().toISOString().slice(0, 10);
  const [ticker, setTicker] = useState("");
  const [name, setName] = useState("");
  const [quantity, setQuantity] = useState("");
  const [cost, setCost] = useState("");
  const [currency, setCurrency] = useState("USD");
  const [date, setDate] = useState(defaultDate ?? today);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Seed the purchase date from the portfolio's inception each time the dialog
  // opens; the user can then override it before saving.
  useEffect(() => {
    if (open) setDate(defaultDate ?? new Date().toISOString().slice(0, 10));
  }, [open, defaultDate]);

  const submit = async () => {
    if (!portfolioId) return;
    const qty = Number(quantity);
    const c = Number(cost);
    if (!ticker.trim()) return setError("Ticker is required");
    if (!(qty > 0)) return setError("Quantity must be positive");
    if (!(c > 0)) return setError("Cost must be positive");
    if (!date) return setError("Purchase date is required");
    setBusy(true);
    setError(null);
    try {
      await postJson(`/api/portfolios/${portfolioId}/holdings`, {
        ticker: ticker.trim().toUpperCase(),
        name: name.trim() || undefined,
        quantity: qty,
        cost: c,
        currency,
        date,
      });
      onAdded();
      onOpenChange(false);
      setTicker("");
      setName("");
      setQuantity("");
      setCost("");
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="dark">
        <DialogHeader>
          <DialogTitle>Add holding</DialogTitle>
          <DialogDescription>
            Recorded as a deposit + buy in the transaction ledger.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-3 py-1">
          <div className="grid grid-cols-2 gap-3">
            <Field label="Ticker">
              <Input
                value={ticker}
                onChange={(e) => setTicker(e.target.value)}
                placeholder="NVDA"
                autoFocus
              />
            </Field>
            <Field label="Currency">
              <CcySelect value={currency} onChange={setCurrency} />
            </Field>
          </div>
          <Field label="Name (optional)">
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="NVIDIA Corp"
            />
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Quantity">
              <Input
                type="number"
                value={quantity}
                onChange={(e) => setQuantity(e.target.value)}
                placeholder="1500"
              />
            </Field>
            <Field label="Cost / unit">
              <Input
                type="number"
                value={cost}
                onChange={(e) => setCost(e.target.value)}
                placeholder="64.50"
              />
            </Field>
          </div>
          <Field label="Purchase date">
            <Input
              type="date"
              value={date}
              max={today}
              onChange={(e) => setDate(e.target.value)}
            />
          </Field>
          {error && <p className="text-sm text-red-400">{error}</p>}
        </div>
        <DialogFooter>
          <Button onClick={submit} disabled={busy} style={accentBtn}>
            {busy ? "Adding…" : "Add holding"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function SellHoldingDialog({
  open,
  onOpenChange,
  portfolioId,
  position,
  onSold,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  portfolioId: string | null;
  /** The position being sold; null while the dialog is closed. */
  position: PositionDetail | null;
  onSold: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="dark">
        {position && (
          // Keyed on the holding so the form re-seeds from fresh props rather
          // than resetting itself in an effect.
          <SellForm
            key={`${position.ticker}:${position.quantity}`}
            portfolioId={portfolioId}
            position={position}
            onSold={onSold}
            onOpenChange={onOpenChange}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function SellForm({
  portfolioId,
  position,
  onSold,
  onOpenChange,
}: {
  portfolioId: string | null;
  position: PositionDetail;
  onSold: () => void;
  onOpenChange: (v: boolean) => void;
}) {
  const today = new Date().toISOString().slice(0, 10);
  // Default to closing the whole position at the last price — the common case.
  // Partial sells are one edit (or one preset button) away.
  const [quantity, setQuantity] = useState(qty(position.quantity));
  const [price, setPrice] = useState(position.last.toFixed(2));
  const [date, setDate] = useState(today);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const sellQty = Number(quantity);
  const px = Number(price);
  const held = position.quantity;
  const partial = sellQty > 0 && sellQty < held;
  const proceeds = sellQty > 0 && px > 0 ? sellQty * px : 0;
  const sym = ccySym(position.ccy);

  const submit = async () => {
    if (!portfolioId) return;
    if (!(sellQty > 0)) return setError("Quantity must be positive");
    if (sellQty > held) return setError(`Only ${qty(held)} held`);
    if (!(px > 0)) return setError("Price must be positive");
    if (!date) return setError("Sale date is required");
    setBusy(true);
    setError(null);
    try {
      await postJson(`/api/portfolios/${portfolioId}/sell`, {
        ticker: position.ticker,
        quantity: sellQty,
        price: px,
        date,
      });
      onSold();
      onOpenChange(false);
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <DialogHeader>
        <DialogTitle>Sell {position.ticker}</DialogTitle>
        <DialogDescription>
          Recorded as a sell + withdrawal in the transaction ledger. Selling the
          full quantity closes the position.
        </DialogDescription>
      </DialogHeader>
      <div className="flex flex-col gap-3 py-1">
        <div className="grid grid-cols-2 gap-3">
          <Field label={`Quantity (${nf(held, 0)} held)`}>
            <Input
              type="number"
              value={quantity}
              max={held}
              onChange={(e) => setQuantity(e.target.value)}
              autoFocus
            />
          </Field>
          <Field label={`Price / unit (${position.ccy})`}>
            <Input
              type="number"
              value={price}
              onChange={(e) => setPrice(e.target.value)}
            />
          </Field>
        </div>
        <div className="flex gap-2">
          {[0.25, 0.5, 0.75, 1].map((fr) => (
            <Button
              key={fr}
              onClick={() => setQuantity(qty(held * fr))}
              style={{
                flex: 1,
                background: "#14161d",
                border: "1px solid rgba(255,255,255,.08)",
                color: "#c5cad6",
                fontSize: 11.5,
              }}
            >
              {fr === 1 ? "All" : `${fr * 100}%`}
            </Button>
          ))}
        </div>
        <Field label="Sale date">
          <Input
            type="date"
            value={date}
            max={today}
            onChange={(e) => setDate(e.target.value)}
          />
        </Field>
        <div
          className="mono"
          style={{
            fontSize: 11.5,
            color: "#9aa1b2",
            display: "flex",
            justifyContent: "space-between",
            gap: 12,
          }}
        >
          <span style={{ whiteSpace: "nowrap" }}>
            {partial ? "Remaining" : "Closes position"}
          </span>
          <span style={{ color: "#e8eaf0", whiteSpace: "nowrap" }}>
            {partial ? `${qty(held - sellQty)} ${position.ticker} · ` : ""}
            {sym + nf(proceeds, 2)} proceeds
          </span>
        </div>
        {error && <p className="text-sm text-red-400">{error}</p>}
      </div>
      <DialogFooter>
        <Button onClick={submit} disabled={busy} style={accentBtn}>
          {busy ? "Selling…" : partial ? "Sell" : "Close position"}
        </Button>
      </DialogFooter>
    </>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label className="pe-lbl">{label}</Label>
      {children}
    </div>
  );
}

function CcySelect({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <Select value={value} onValueChange={(v) => v && onChange(v)}>
      <SelectTrigger>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {CURRENCIES.map((c) => (
          <SelectItem key={c} value={c}>
            {c}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
