import assert from "node:assert/strict";
import test from "node:test";

import { render, validateSource } from "../scripts/render-testers.mjs";

function report(id, { publish = false } = {}) {
  return {
    approved_at: "2026-09-09T12:00:00.000Z",
    report: {
      schema_version: "cameo-hardware-report/v1",
      report_id: id,
      generated_at_unix: 1788955200,
      artifact: { cameo_version: "0.1.0", iso_build_id: "cameo-test" },
      platform: { os: "linux", architecture: "x86_64" },
      hardware: {
        status: "observed",
        gpus: [{
          model: "AMD Radeon RX 6700 XT",
          vendor: "AMD",
          pci_id: "1002:73df",
          gfx_target: "gfx1031",
          vram_total_bytes: 12884901888,
          is_apu: false,
          cameo_tier: 2,
        }],
      },
      outcomes: {
        boot: { result: "passed", evidence_source: "observed" },
        gpu_detection: { result: "passed", evidence_source: "observed" },
        inference: { result: "passed", evidence_source: "observed" },
      },
      privacy: {
        allowlisted_fields_only: true,
        hostname: false,
        username: false,
        ip_or_mac: false,
        serial_numbers: false,
        credentials: false,
        raw_logs: false,
        prompts_or_model_output: false,
      },
    },
    credit_request: {
      publish,
      ...(publish ? { display_name: "Tester" } : {}),
      permanence_acknowledged: publish,
    },
  };
}

test("approved evidence generates an aggregate matrix and consented credits", () => {
  const source = {
    schema: "cameo-approved-hardware/v1",
    release_id: "unreleased",
    cutoff_at: null,
    approved: [
      report("chr_000000000000000000000001", { publish: true }),
      report("chr_000000000000000000000002"),
    ],
  };
  validateSource(source);
  const { roster, matrix } = render(source);
  assert.equal(roster.testers.length, 1);
  assert.equal(roster.testers[0].name, "Tester");
  assert.equal(matrix.entries.length, 1);
  assert.equal(matrix.entries[0].report_count, 2);
  assert.equal(matrix.entries[0].inference.passed, 2);
  assert.equal(matrix.evidence_class, "approved_community_reports");
});

test("duplicate report IDs are rejected", () => {
  const item = report("chr_000000000000000000000003");
  assert.throws(() => validateSource({
    schema: "cameo-approved-hardware/v1",
    release_id: "unreleased",
    cutoff_at: null,
    approved: [item, structuredClone(item)],
  }), /duplicate approved report/);
});
