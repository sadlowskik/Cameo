#!/usr/bin/env node

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const retiredDocuments = [
  "CAMEO_PROJECT_PLAN.md",
  "IMPLEMENTATION_LEDGER.md",
  "docs/api.md",
  "docs/audit-findings.md",
  "docs/cloud-validation.md",
  "docs/completion-cameo.md",
  "docs/completion-field.md",
  "docs/completion-issues.md",
  "docs/completion-knossos.md",
  "docs/completion-plan.md",
  "docs/definition-of-done.md",
  "docs/product-spec.md",
  "docs/production-audit.md",
  "docs/remediation-plan.md",
];

const ignoredDirectories = new Set([
  ".git",
  ".agents",
  ".claude",
  ".codex",
  ".pytest_cache",
  ".venv",
  "node_modules",
  "target",
  "build",
  "dist",
]);

function findMarkdown(directory) {
  const found = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isSymbolicLink()) continue;
    const path = resolve(directory, entry.name);
    if (entry.isDirectory() && !ignoredDirectories.has(entry.name)) {
      found.push(...findMarkdown(path));
    } else if (entry.isFile() && entry.name.toLowerCase().endsWith(".md")) {
      found.push(relative(root, path));
    }
  }
  return found;
}

const markdownFiles = findMarkdown(root);
const failures = [];

function localTarget(rawTarget) {
  let target = rawTarget.trim();
  if (target.startsWith("<") && target.endsWith(">")) {
    target = target.slice(1, -1);
  }
  if (
    target === "" ||
    target.startsWith("#") ||
    /^(?:https?:|mailto:|data:|javascript:)/i.test(target)
  ) {
    return null;
  }
  target = target.split("#", 1)[0].split("?", 1)[0];
  try {
    return decodeURIComponent(target);
  } catch {
    return target;
  }
}

for (const file of markdownFiles) {
  const absolute = resolve(root, file);
  const source = readFileSync(absolute, "utf8");

  for (const retired of retiredDocuments) {
    if (source.includes(retired)) {
      failures.push(`${file}: references retired document ${retired}`);
    }
  }

  const targets = [];
  for (const match of source.matchAll(/!?\[[^\]]*\]\(([^)]+)\)/g)) {
    targets.push(match[1].replace(/\s+["'][^"']*["']\s*$/, ""));
  }
  for (const match of source.matchAll(/^\s*\[[^\]]+\]:\s*(\S+)/gm)) {
    targets.push(match[1]);
  }

  for (const rawTarget of targets) {
    const target = localTarget(rawTarget);
    if (target === null) continue;
    const destination = target.startsWith("/")
      ? resolve(root, `.${target}`)
      : resolve(dirname(absolute), target);
    if (!destination.startsWith(root) || !existsSync(destination)) {
      failures.push(`${file}: missing local link ${rawTarget}`);
    }
  }
}

if (failures.length > 0) {
  console.error("Documentation link check failed:");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`Documentation links OK (${markdownFiles.length} Markdown files).`);
