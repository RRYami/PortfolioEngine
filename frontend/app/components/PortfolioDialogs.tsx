"use client";

import { useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
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
    const msg = await r.text().catch(() => "");
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
  const [ticker, setTicker] = useState("");
  const [name, setName] = useState("");
  const [quantity, setQuantity] = useState("");
  const [cost, setCost] = useState("");
  const [currency, setCurrency] = useState("USD");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    if (!portfolioId) return;
    const qty = Number(quantity);
    const c = Number(cost);
    if (!ticker.trim()) return setError("Ticker is required");
    if (!(qty > 0)) return setError("Quantity must be positive");
    if (!(c > 0)) return setError("Cost must be positive");
    setBusy(true);
    setError(null);
    try {
      await postJson(`/api/portfolios/${portfolioId}/holdings`, {
        ticker: ticker.trim().toUpperCase(),
        name: name.trim() || undefined,
        quantity: qty,
        cost: c,
        currency,
        date: defaultDate,
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
