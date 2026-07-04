export interface PortfolioSummary {
  id: string;
  name: string;
  baseCcy: string;
  lotMethod: string;
  inceptionDate: string;
}

export const CURRENCIES = ["USD", "EUR", "GBP", "JPY", "CHF"] as const;

export const LOT_METHODS: { value: string; label: string }[] = [
  { value: "fifo", label: "FIFO" },
  { value: "lifo", label: "LIFO" },
  { value: "highest_cost", label: "Highest cost" },
  { value: "lowest_cost", label: "Lowest cost" },
  { value: "average_cost", label: "Average cost" },
];
