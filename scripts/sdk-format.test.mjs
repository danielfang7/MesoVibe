// The SDK's shared formats (sdk/src/format.ts): what the session strip and every view print.
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  formatCompact,
  formatDigits,
  formatDuration,
  formatMinutes,
  formatPercent,
} from "../sdk/src/format.ts";

test("full digits group by thousands", () => {
  assert.equal(formatDigits(0), "0");
  assert.equal(formatDigits(999), "999");
  assert.equal(formatDigits(1_847_302), "1,847,302");
  assert.equal(formatDigits(-15_500), "-15,500");
  assert.equal(formatDigits(356_916.4), "356,916");
});

test("compact numbers keep three significant digits and drop trailing zeros", () => {
  assert.equal(formatCompact(950), "950");
  assert.equal(formatCompact(1_000), "1K");
  assert.equal(formatCompact(48_210), "48.2K");
  assert.equal(formatCompact(127_440), "127K");
  assert.equal(formatCompact(1_847_302), "1.85M");
  assert.equal(formatCompact(2_000_000_000), "2B");
});

// The hero EXP/hr must read at its real size. Fails if a whole number's own zeros are stripped
// (140K shown as "14K", a tenfold misread) or rounding prints "1000K".
test("compact numbers keep the zeros of whole numbers, and rounding carries into the next unit", () => {
  assert.equal(formatCompact(140_400), "140K");
  assert.equal(formatCompact(100_000), "100K");
  assert.equal(formatCompact(230_000), "230K");
  assert.equal(formatCompact(250_000_000), "250M");
  assert.equal(formatCompact(10_000), "10K");
  assert.equal(formatCompact(1_500_000), "1.5M");
  assert.equal(formatCompact(999_600), "1M");
  assert.equal(formatCompact(999.6), "1K");
  assert.equal(formatCompact(-140_400), "-140K");
});

test("durations and minutes read the way players say them", () => {
  assert.equal(formatDuration(42_000), "42s");
  assert.equal(formatDuration(300_000), "5m");
  assert.equal(formatDuration(7_800_000), "2h 10m");
  assert.equal(formatDuration(-5), "0s");
  assert.equal(formatMinutes(0.4), "under a minute");
  assert.equal(formatMinutes(23.4), "23 min");
  assert.equal(formatMinutes(120), "2h");
  assert.equal(formatMinutes(125), "2h 5m");
});

test("percent hundredths print with two decimals", () => {
  assert.equal(formatPercent(6420), "64.20%");
  assert.equal(formatPercent(5), "0.05%");
  assert.equal(formatPercent(10_000), "100.00%");
});
