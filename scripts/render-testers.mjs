#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { validateSubmission } from "../functions/api/hardware-reports.js";

const root = resolve(import.meta.dirname, "..");
const sourcePath = resolve(root, "testers/approved-reports.json");
const rosterPath = resolve(root, "testers/roster.json");
const siteRosterPath = resolve(root, "site/testers.json");
const matrixPath = resolve(root, "site/hardware-matrix.json");
const check = process.argv.includes("--check");
const consent = "Names appear only with explicit consent to permanent public credit in the site, repository, and immutable release artifacts.";
const disclaimer = "Approved community reports are compatibility evidence, not hardware certification, a warranty, or a release-readiness claim.";

function exactKeys(value, keys) {
  return value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).length === keys.length
    && keys.every((key) => Object.hasOwn(value, key));
}

function outcomeCounts() {
  return { passed: 0, failed: 0, degraded: 0, not_tested: 0 };
}

function isoFromUnix(seconds) {
  return new Date(seconds * 1000).toISOString();
}

function bounded(value, max) {
  const points = [...String(value)];
  return points.length <= max ? points.join("") : `${points.slice(0, max - 1).join("")}…`;
}

function proofSummary(outcomes) {
  const label = (value) => value.replace("_", " ");
  return bounded(
    `Boot ${label(outcomes.boot.result)}; GPU detection ${label(outcomes.gpu_detection.result)}; starter-model inference ${label(outcomes.inference.result)}.`,
    500,
  );
}

function validateSource(source) {
  if (!exactKeys(source, ["schema", "release_id", "cutoff_at", "approved"])
    || source.schema !== "cameo-approved-hardware/v1"
    || typeof source.release_id !== "string"
    || source.release_id.length < 1
    || !(source.cutoff_at === null
      || (typeof source.cutoff_at === "string" && new Date(source.cutoff_at).toISOString() === source.cutoff_at))
    || !Array.isArray(source.approved)) {
    throw new Error("testers/approved-reports.json does not satisfy cameo-approved-hardware/v1");
  }
  const ids = new Set();
  for (const item of source.approved) {
    if (!exactKeys(item, ["approved_at", "report", "credit_request"])
      || typeof item.approved_at !== "string"
      || new Date(item.approved_at).toISOString() !== item.approved_at
      || !validateSubmission({
        schema_version: "cameo-hardware-submission/v1",
        report: item.report,
        credit_request: item.credit_request,
      })) {
      throw new Error("approved hardware source contains an invalid record");
    }
    if (ids.has(item.report.report_id)) {
      throw new Error(`duplicate approved report ${item.report.report_id}`);
    }
    ids.add(item.report.report_id);
  }
}

function render(source) {
  const approved = [...source.approved].sort((a, b) =>
    a.approved_at.localeCompare(b.approved_at)
      || a.report.report_id.localeCompare(b.report.report_id));
  const testers = approved
    .filter((item) => item.credit_request.publish && item.credit_request.permanence_acknowledged)
    .map((item) => {
      const gpus = item.report.hardware.gpus.map((gpu) => gpu.model);
      return {
        name: item.credit_request.display_name,
        date: item.approved_at.slice(0, 10),
        machine: bounded(gpus.length ? gpus.join(" + ") : "No AMD GPU recorded", 160),
        gpu: bounded(gpus[0] || "No AMD GPU recorded", 160),
        proved: proofSummary(item.report.outcomes),
      };
    });
  const roster = {
    schema: "cameo-testers/v1",
    consent,
    release_id: source.release_id,
    cutoff_at: source.cutoff_at,
    testers,
  };

  const groups = new Map();
  for (const item of approved) {
    const seen = new Set();
    for (const gpu of item.report.hardware.gpus) {
      const key = [gpu.pci_id.toLowerCase(), gpu.gfx_target || "unknown", gpu.model, gpu.is_apu, gpu.cameo_tier].join("|");
      if (seen.has(key)) continue;
      seen.add(key);
      let entry = groups.get(key);
      if (!entry) {
        entry = {
          key,
          model: gpu.model,
          pci_id: gpu.pci_id.toLowerCase(),
          ...(gpu.gfx_target ? { gfx_target: gpu.gfx_target } : {}),
          is_apu: gpu.is_apu,
          cameo_tier: gpu.cameo_tier,
          report_count: 0,
          boot: outcomeCounts(),
          gpu_detection: outcomeCounts(),
          inference: outcomeCounts(),
          last_reported_at: isoFromUnix(item.report.generated_at_unix),
          cameo_versions: [],
        };
        groups.set(key, entry);
      }
      entry.report_count += 1;
      entry.boot[item.report.outcomes.boot.result] += 1;
      entry.gpu_detection[item.report.outcomes.gpu_detection.result] += 1;
      entry.inference[item.report.outcomes.inference.result] += 1;
      const reportedAt = isoFromUnix(item.report.generated_at_unix);
      if (reportedAt > entry.last_reported_at) entry.last_reported_at = reportedAt;
      if (!entry.cameo_versions.includes(item.report.artifact.cameo_version)) {
        entry.cameo_versions.push(item.report.artifact.cameo_version);
        entry.cameo_versions.sort();
      }
    }
  }
  const matrix = {
    schema: "cameo-hardware-matrix/v1",
    evidence_class: "approved_community_reports",
    disclaimer,
    entries: [...groups.values()].sort((a, b) => a.key.localeCompare(b.key)),
  };
  return { roster, matrix };
}

function serialized(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

function ensure(path, expected) {
  if (check) {
    if (readFileSync(path, "utf8") !== expected) {
      throw new Error(`${path.slice(root.length + 1).replaceAll("\\", "/")} is stale; run node scripts/render-testers.mjs`);
    }
  } else {
    writeFileSync(path, expected);
  }
}

export { render, validateSource };

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const source = JSON.parse(readFileSync(sourcePath, "utf8"));
  validateSource(source);
  const { roster, matrix } = render(source);
  ensure(rosterPath, serialized(roster));
  ensure(siteRosterPath, serialized(roster));
  ensure(matrixPath, serialized(matrix));
  console.log(check
    ? "Approved reports, tester credits, and hardware matrix are synchronized."
    : "Rendered approved reports into tester credits and the public hardware matrix.");
}
