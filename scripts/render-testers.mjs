#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const sourcePath = resolve(root, "testers/roster.json");
const sitePath = resolve(root, "site/testers.json");
const check = process.argv.includes("--check");

const roster = JSON.parse(readFileSync(sourcePath, "utf8"));
if (roster.schema !== "cameo-testers/v1" || !Array.isArray(roster.testers)) {
  throw new Error("testers/roster.json does not satisfy cameo-testers/v1");
}
const rendered = `${JSON.stringify(roster, null, 2)}\n`;

if (check) {
  if (readFileSync(sitePath, "utf8") !== rendered) {
    throw new Error("site/testers.json is stale; run node scripts/render-testers.mjs");
  }
  console.log("Tester roster is synchronized.");
} else {
  writeFileSync(sitePath, rendered);
  console.log("Rendered testers/roster.json into site/testers.json.");
}
