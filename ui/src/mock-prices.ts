// The development stand-in's Free Market prices (see mock.ts), so the Prices tab can be looked at
// and tested without the price service. Real game items with made-up prices; the search is a plain
// name match, not the host's (app/src/prices.rs).
import type { Host, PriceDetail, PriceRow, PriceStatus, World } from "./ipc.js";

interface MockItem {
  id: number;
  name: string;
  npc: number | null;
  /** Windia's usual price; Zenith's is half. */
  usual: number | null;
  change: number | null;
  untradeable?: boolean;
}

const ITEMS: MockItem[] = [
  { id: 2070006, name: "Ilbi Throwing Stars", npc: 20_000, usual: 21_000_000, change: 0.08 },
  { id: 2070000, name: "Subi Throwing Stars", npc: 250, usual: 1_200, change: -0.02 },
  { id: 1082000, name: "Work Gloves", npc: 1_200, usual: 1_500_000, change: 0.25 },
  { id: 1082002, name: "Steel Fingerless Gloves", npc: 1_800, usual: 85_000, change: -0.1 },
  { id: 2000000, name: "Red Potion", npc: 5, usual: null, change: null },
  { id: 2000002, name: "White Potion", npc: 35, usual: 300, change: 0 },
  { id: 2010000, name: "Roger's Apple", npc: null, usual: null, change: null, untradeable: true },
];
const HISTORY_DAYS = 30;
const HOUR_MS = 60 * 60 * 1000;

type PriceHost = Pick<
  Host,
  "priceStatus" | "onPriceStatus" | "priceSearch" | "priceItem" | "priceRows" | "priceMovers" | "priceRecent" | "setPriceRecent" | "openPricePage"
>;

/** The price calls of the stand-in host. `pricesold` has two-day-old prices and no connection; `pricesoffline` never downloaded any. */
export function mockPrices(scenario: string): PriceHost {
  const now = Date.now();
  const at = (ms: number) => new Date(ms).toISOString();
  const status: PriceStatus =
    scenario === "pricesoffline"
      ? { downloading: false, failed: true, updated: null, checkedAtMs: null }
      : scenario === "pricesold"
        ? { downloading: false, failed: true, updated: { windia: at(now - 48 * HOUR_MS), zenith: at(now - 48 * HOUR_MS) }, checkedAtMs: null }
        : { downloading: false, failed: false, updated: { windia: at(now - 2 * HOUR_MS), zenith: at(now - 2 * HOUR_MS) }, checkedAtMs: now };
  const hasPrices = status.updated !== null;
  let recent: number[] = [];
  /** Pages the panel asked to open, for the panel tests (`window.__mockPricePages`). */
  const pages: { id: number | null; world: World }[] = [];
  (window as unknown as { __mockPricePages: typeof pages }).__mockPricePages = pages;

  const scale = (world: World) => (world === "zenith" ? 0.5 : 1);
  const usual = (item: MockItem, world: World) => (hasPrices && item.usual !== null ? item.usual * scale(world) : null);
  const row = (item: MockItem, world: World): PriceRow => ({
    id: item.id,
    name: item.name,
    gender: null,
    usual: usual(item, world),
    change: hasPrices ? item.change : null,
    npc: item.npc,
  });
  const days = Array.from({ length: HISTORY_DAYS }, (_, i) => at(now - (HISTORY_DAYS - 1 - i) * 24 * HOUR_MS).slice(0, 10));
  const history = (price: number | null): (number | null)[] =>
    Array.from({ length: HISTORY_DAYS }, (_, i) => {
      if (price === null || i === 11) return null;
      if (i === HISTORY_DAYS - 1) return price;
      return Math.round(price * (0.85 + (0.15 * i) / (HISTORY_DAYS - 1) + 0.03 * Math.sin(i)));
    });
  const byId = (id: number) => ITEMS.find((item) => item.id === id);

  return {
    priceStatus: async () => status,
    onPriceStatus: async () => {},
    priceSearch: async (query, world) => {
      const q = query.trim().toLowerCase().replace(/\s+/g, " ");
      const found = ITEMS.filter((item) => item.name.toLowerCase().includes(q)).sort(
        (a, b) => Number(!a.name.toLowerCase().startsWith(q)) - Number(!b.name.toLowerCase().startsWith(q)) || Number(a.usual === null) - Number(b.usual === null),
      );
      return { rows: found.slice(0, 8).map((item) => row(item, world)), total: found.length };
    },
    priceItem: async (id, world): Promise<PriceDetail | null> => {
      const item = byId(id);
      if (item === undefined) return null;
      const price = usual(item, world);
      return {
        ...row(item, world),
        low7d: price === null ? null : Math.round(price * 0.93),
        high7d: price,
        sales: price === null ? null : 412,
        listings: price === null ? null : 96,
        untradeable: item.untradeable ?? false,
        history: history(price),
        historyDays: days,
        page: price !== null,
      };
    },
    priceRows: async (ids, world) => ids.flatMap((id) => (byId(id) ? [row(byId(id) as MockItem, world)] : [])),
    priceMovers: async (world) => {
      if (!hasPrices) return { up: [], down: [] };
      const moved = ITEMS.filter((item) => item.change !== null && item.change !== 0);
      const up = moved.filter((item) => (item.change ?? 0) > 0).sort((a, b) => (b.change ?? 0) - (a.change ?? 0));
      const down = moved.filter((item) => (item.change ?? 0) < 0).sort((a, b) => (a.change ?? 0) - (b.change ?? 0));
      return { up: up.map((item) => row(item, world)), down: down.map((item) => row(item, world)) };
    },
    priceRecent: async () => recent,
    setPriceRecent: async (ids) => {
      recent = ids.slice(0, 8);
    },
    openPricePage: async (id, world) => {
      pages.push({ id, world });
    },
  };
}
