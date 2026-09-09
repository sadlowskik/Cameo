#!/usr/bin/env node

import { readFileSync, renameSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

import { validateSubmission } from "../functions/api/hardware-reports.js";

const root = resolve(import.meta.dirname, "..");
const sourcePath = resolve(root, "testers/approved-reports.json");
const renderPath = resolve(root, "scripts/render-testers.mjs");
const database = "cameo-hardware-reports";
const reportIdPattern = /^chr_[0-9a-f]{24}$/;

function usage(message) {
  if (message) console.error(`error: ${message}\n`);
  console.error(`Usage:
  node scripts/review-hardware-reports.mjs list
  node scripts/review-hardware-reports.mjs export <file>
  node scripts/review-hardware-reports.mjs approve <chr_report_id>
  node scripts/review-hardware-reports.mjs reject <chr_report_id>

The tool uses the authenticated Wrangler CLI from PATH. Set CAMEO_WRANGLER to
an explicit wrangler, wrangler.cmd, or wrangler.js path when needed. Approval
updates the git-reviewed source and generated public files but never deploys.`);
  process.exit(message ? 2 : 0);
}

function commandForWrangler(args) {
  const configured = process.env.CAMEO_WRANGLER || "wrangler";
  if (/\.m?js$/i.test(configured)) {
    return { command: process.execPath, args: [configured, ...args], shell: false };
  }
  return { command: configured, args, shell: process.platform === "win32" };
}

function runWrangler(args) {
  const invocation = commandForWrangler(args);
  const result = spawnSync(invocation.command, invocation.args, {
    cwd: root,
    encoding: "utf8",
    shell: invocation.shell,
    maxBuffer: 4 * 1024 * 1024,
  });
  if (result.error) throw new Error(`could not run Wrangler: ${result.error.message}`);
  if (result.status !== 0) {
    throw new Error(`Wrangler exited ${result.status}: ${(result.stderr || result.stdout).trim()}`);
  }
  try {
    return JSON.parse(result.stdout);
  } catch {
    throw new Error("Wrangler did not return JSON; use Wrangler 4.x and authenticate first");
  }
}

function execute(sql) {
  const response = runWrangler([
    "d1", "execute", database, "--remote", "--command", sql, "--json",
  ]);
  if (!Array.isArray(response) || response.some((part) => part.success !== true)) {
    throw new Error("D1 returned an unsuccessful response");
  }
  return response;
}

function rows(response) {
  return response.flatMap((part) => Array.isArray(part.results) ? part.results : []);
}

function pendingRows() {
  return rows(execute(
    "SELECT report_id, submitted_at_unix, report_json, credit_publish, credit_display_name, status "
    + "FROM pending_hardware_reports WHERE status = 'pending' "
    + "ORDER BY submitted_at_unix ASC LIMIT 100",
  ));
}

function requireReportId(value) {
  if (!reportIdPattern.test(value || "")) usage("report ID must match chr_ followed by 24 lowercase hex characters");
  return value;
}

function fetchRow(reportId) {
  const found = rows(execute(
    "SELECT report_id, submitted_at_unix, report_json, credit_publish, credit_display_name, status "
    + `FROM pending_hardware_reports WHERE report_id = '${reportId}' LIMIT 1`,
  ));
  if (found.length !== 1) throw new Error(`report ${reportId} was not found`);
  return found[0];
}

function submissionFromRow(row) {
  let report;
  try {
    report = JSON.parse(row.report_json);
  } catch {
    throw new Error(`stored report ${row.report_id} contains invalid JSON`);
  }
  const publish = row.credit_publish === 1 || row.credit_publish === true;
  const creditRequest = {
    publish,
    ...(publish ? { display_name: row.credit_display_name } : {}),
    permanence_acknowledged: publish,
  };
  const submission = {
    schema_version: "cameo-hardware-submission/v1",
    report,
    credit_request: creditRequest,
  };
  if (report.report_id !== row.report_id || !validateSubmission(submission)) {
    throw new Error(`stored report ${row.report_id} fails the current submission contract`);
  }
  return submission;
}

function writeApproved(submission) {
  const source = JSON.parse(readFileSync(sourcePath, "utf8"));
  if (source.schema !== "cameo-approved-hardware/v1" || !Array.isArray(source.approved)) {
    throw new Error("testers/approved-reports.json is not a valid approved-report source");
  }
  const { note: _note, error_excerpt: _errorExcerpt, ...publicReport } = submission.report;
  const publicRecord = {
    report: publicReport,
    credit_request: submission.credit_request,
  };
  const existing = source.approved.find((item) => item.report?.report_id === submission.report.report_id);
  if (existing && (JSON.stringify(existing.report) !== JSON.stringify(publicRecord.report)
    || JSON.stringify(existing.credit_request) !== JSON.stringify(publicRecord.credit_request))) {
    throw new Error(`approved source conflicts with stored report ${submission.report.report_id}`);
  }
  if (!existing) {
    // Free-text is useful to the reviewer but is never promoted to the public,
    // version-controlled evidence set. The structured allowlist is sufficient.
    source.approved.push({
      approved_at: new Date().toISOString(),
      ...publicRecord,
    });
    source.approved.sort((a, b) => a.approved_at.localeCompare(b.approved_at)
      || a.report.report_id.localeCompare(b.report.report_id));
    const temporary = `${sourcePath}.tmp`;
    writeFileSync(temporary, `${JSON.stringify(source, null, 2)}\n`, { flag: "wx" });
    renameSync(temporary, sourcePath);
  }
  const rendered = spawnSync(process.execPath, [renderPath], {
    cwd: root,
    encoding: "utf8",
    shell: false,
  });
  if (rendered.error || rendered.status !== 0) {
    throw new Error(`approved source was saved, but rendering failed: ${(rendered.stderr || rendered.error?.message || "unknown error").trim()}`);
  }
  return !existing;
}

function decide(reportId, status) {
  const response = execute(
    `UPDATE pending_hardware_reports SET status = '${status}', reviewed_at_unix = ${Math.floor(Date.now() / 1000)} `
    + `WHERE report_id = '${reportId}' AND status = 'pending'`,
  );
  const changes = response.reduce((sum, part) => sum + Number(part.meta?.changes || 0), 0);
  if (changes !== 1) {
    const current = fetchRow(reportId);
    if (current.status !== status) {
      throw new Error(`report is ${current.status}; it was not changed to ${status}`);
    }
  }
}

function exportQueue(path) {
  const queue = pendingRows().map((row) => ({
    report_id: row.report_id,
    submitted_at: new Date(row.submitted_at_unix * 1000).toISOString(),
    submission: submissionFromRow(row),
  }));
  const output = `${JSON.stringify({ schema: "cameo-review-queue/v1", pending: queue }, null, 2)}\n`;
  if (!path) {
    process.stdout.write(output);
    return;
  }
  writeFileSync(resolve(path), output, { flag: "wx", mode: 0o600 });
  console.log(`Exported ${queue.length} pending report(s) to ${resolve(path)}; existing files are never overwritten.`);
}

const [action, operand, ...extra] = process.argv.slice(2);
if (!action || action === "--help" || action === "-h") usage();
if (extra.length) usage("too many arguments");

try {
  if (action === "list") {
    if (operand) usage("list does not take a file");
    exportQueue();
  } else if (action === "export") {
    if (!operand) usage("export requires a new output file");
    exportQueue(operand);
  } else if (action === "approve") {
    const reportId = requireReportId(operand);
    const row = fetchRow(reportId);
    if (!new Set(["pending", "approved"]).has(row.status)) {
      throw new Error(`report is already ${row.status}`);
    }
    const added = writeApproved(submissionFromRow(row));
    decide(reportId, "approved");
    console.log(`${reportId} approved${added ? "; canonical evidence and generated files updated" : "; existing canonical evidence retained"}. Nothing was deployed.`);
  } else if (action === "reject") {
    const reportId = requireReportId(operand);
    decide(reportId, "rejected");
    console.log(`${reportId} rejected; nothing was published or deployed.`);
  } else {
    usage(`unknown action ${action}`);
  }
} catch (error) {
  console.error(`review failed: ${error.message}`);
  process.exit(1);
}
