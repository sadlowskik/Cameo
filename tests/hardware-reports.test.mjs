import assert from "node:assert/strict";
import test from "node:test";

import { onRequest, validateSubmission } from "../functions/api/hardware-reports.js";

function sample() {
  return {
    schema_version: "cameo-hardware-submission/v1",
    report: {
      schema_version: "cameo-hardware-report/v1",
      report_id: "chr_0123456789abcdef01234567",
      generated_at_unix: Math.floor(Date.now() / 1000),
      artifact: { cameo_version: "0.2.0-beta.4" },
      platform: { os: "linux", architecture: "x86_64" },
      hardware: {
        status: "observed",
        gpus: [{
          model: "Radeon RX 6700 XT",
          vendor: "AMD",
          pci_id: "1002:73df",
          gfx_target: "gfx1031",
          is_apu: false,
          cameo_tier: 2,
        }],
      },
      outcomes: {
        boot: { result: "passed", evidence_source: "tester_asserted" },
        gpu_detection: { result: "passed", evidence_source: "observed" },
        inference: { result: "not_tested", evidence_source: "unavailable" },
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
    credit_request: { publish: false, permanence_acknowledged: false },
  };
}

test("accepts a strict anonymous CLI submission", () => {
  assert.equal(validateSubmission(sample(), 1_800_000_000), true);
});

test("accepts credit only with a bounded display name and permanence consent", () => {
  const value = sample();
  value.credit_request = {
    publish: true,
    display_name: "@tester",
    permanence_acknowledged: true,
  };
  assert.equal(validateSubmission(value, 1_800_000_000), true);
  value.credit_request.permanence_acknowledged = false;
  assert.equal(validateSubmission(value, 1_800_000_000), false);
});

test("rejects non-allowlisted fields and privacy contradictions", () => {
  const extra = sample();
  extra.report.hostname = "private-host";
  assert.equal(validateSubmission(extra, 1_800_000_000), false);

  const privacy = sample();
  privacy.report.privacy.hostname = true;
  assert.equal(validateSubmission(privacy, 1_800_000_000), false);
});

test("rejects controls and implausibly future-dated reports", () => {
  const controls = sample();
  controls.report.note = "hello\u001b[31m";
  assert.equal(validateSubmission(controls, 1_800_000_000), false);

  const future = sample();
  future.report.generated_at_unix = 1_800_086_401;
  assert.equal(validateSubmission(future, 1_800_000_000), false);
});

class FakeStatement {
  constructor(database, sql) {
    this.database = database;
    this.sql = sql;
    this.values = [];
  }

  bind(...values) {
    this.values = values;
    return this;
  }

  async run() {
    if (this.sql.includes("INSERT INTO pending_hardware_reports")) {
      this.database.inserted = this.values;
    }
    return { success: true };
  }

  async first(column) {
    assert.equal(column, "request_count");
    this.database.rateCount += 1;
    return this.database.rateCount;
  }
}

class FakeDatabase {
  constructor() {
    this.rateCount = 0;
    this.inserted = null;
  }

  prepare(sql) {
    return new FakeStatement(this, sql);
  }
}

test("POST stores only a pending report and returns an opaque receipt", async () => {
  const database = new FakeDatabase();
  const value = sample();
  const result = await onRequest({
    request: new Request("https://cameoconstruct.xyz/api/hardware-reports", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "CF-Connecting-IP": "192.0.2.1",
      },
      body: JSON.stringify(value),
    }),
    env: { REPORTS: database, RATE_LIMIT_SALT: "test-only-secret" },
  });
  assert.equal(result.status, 202);
  assert.equal(database.inserted[0], value.report.report_id);
  assert.equal(database.inserted[5], undefined);
  assert.deepEqual(await result.json(), {
    status: "pending_moderation",
    receipt: value.report.report_id,
  });
});

test("rate limiting covers the whole intake path and advertises retry timing", async () => {
  const database = new FakeDatabase();
  let result;
  for (let index = 0; index < 11; index += 1) {
    const value = sample();
    value.report.report_id = `chr_${index.toString(16).padStart(24, "0")}`;
    result = await onRequest({
      request: new Request("https://cameoconstruct.xyz/api/hardware-reports", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "CF-Connecting-IP": "192.0.2.2",
        },
        body: JSON.stringify(value),
      }),
      env: { REPORTS: database, RATE_LIMIT_SALT: "test-only-secret" },
    });
  }
  assert.equal(result.status, 429);
  assert.equal(result.headers.get("Retry-After"), "3600");
  assert.deepEqual(await result.json(), { error: "rate_limited" });
});
