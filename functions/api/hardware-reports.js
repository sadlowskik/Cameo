const MAX_BODY_BYTES = 128 * 1024;
const RATE_LIMIT_PER_HOUR = 10;
const REPORT_SCHEMA = "cameo-hardware-report/v1";
const SUBMISSION_SCHEMA = "cameo-hardware-submission/v1";

const jsonHeaders = {
  "Content-Type": "application/json; charset=utf-8",
  "Cache-Control": "no-store",
  "X-Content-Type-Options": "nosniff",
};

function response(status, body, extraHeaders = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { ...jsonHeaders, ...extraHeaders },
  });
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function exactKeys(value, required, optional = []) {
  if (!isObject(value)) return false;
  const allowed = new Set([...required, ...optional]);
  return required.every((key) => Object.hasOwn(value, key))
    && Object.keys(value).every((key) => allowed.has(key));
}

function boundedString(value, max, pattern, allowWhitespaceControls = false) {
  if (typeof value !== "string" || value.length < 1 || [...value].length > max) return false;
  if (allowWhitespaceControls) {
    if (/[^\P{C}\n\t]/u.test(value)) return false;
  } else if (/\p{C}/u.test(value)) {
    return false;
  }
  return pattern ? pattern.test(value) : true;
}

function optionalString(object, key, max, pattern, allowWhitespaceControls = false) {
  return !Object.hasOwn(object, key)
    || boundedString(object[key], max, pattern, allowWhitespaceControls);
}

function safeInteger(value, minimum = 0, maximum = Number.MAX_SAFE_INTEGER) {
  return Number.isSafeInteger(value) && value >= minimum && value <= maximum;
}

function validArtifact(value) {
  const optional = [
    "iso_build_id", "edition", "source_revision", "source_dirty",
    "knossos_revision", "arch_snapshot", "rocm_cli_version",
  ];
  return exactKeys(value, ["cameo_version"], optional)
    && boundedString(value.cameo_version, 64)
    && optionalString(value, "iso_build_id", 128)
    && optionalString(value, "edition", 32)
    && optionalString(value, "source_revision", 64)
    && (!Object.hasOwn(value, "source_dirty") || typeof value.source_dirty === "boolean")
    && optionalString(value, "knossos_revision", 64)
    && optionalString(value, "arch_snapshot", 32)
    && optionalString(value, "rocm_cli_version", 64);
}

function validPlatform(value) {
  return exactKeys(value, ["os", "architecture"], ["kernel_version", "cpu_model"])
    && boundedString(value.os, 32)
    && boundedString(value.architecture, 32)
    && optionalString(value, "kernel_version", 128)
    && optionalString(value, "cpu_model", 160);
}

function validGpu(value) {
  return exactKeys(
    value,
    ["model", "vendor", "pci_id", "is_apu", "cameo_tier"],
    ["gfx_target", "vram_total_bytes", "driver_version"],
  )
    && boundedString(value.model, 200)
    && boundedString(value.vendor, 32)
    && boundedString(value.pci_id, 9, /^[0-9a-f]{4}:[0-9a-f]{4}$/i)
    && optionalString(value, "gfx_target", 32, /^gfx[0-9a-z]+$/)
    && (!Object.hasOwn(value, "vram_total_bytes") || safeInteger(value.vram_total_bytes))
    && typeof value.is_apu === "boolean"
    && optionalString(value, "driver_version", 128)
    && safeInteger(value.cameo_tier, 1, 3);
}

function validHardware(value) {
  return exactKeys(
    value,
    ["status", "gpus"],
    ["system_ram_total_bytes", "system_ram_available_bytes"],
  )
    && ["observed", "unverified"].includes(value.status)
    && Array.isArray(value.gpus)
    && value.gpus.length <= 32
    && value.gpus.every(validGpu)
    && (!Object.hasOwn(value, "system_ram_total_bytes")
      || safeInteger(value.system_ram_total_bytes))
    && (!Object.hasOwn(value, "system_ram_available_bytes")
      || safeInteger(value.system_ram_available_bytes));
}

function validOutcome(value) {
  return exactKeys(value, ["result", "evidence_source"])
    && ["passed", "failed", "degraded", "not_tested"].includes(value.result)
    && ["observed", "tester_asserted", "unavailable"].includes(value.evidence_source);
}

function validOutcomes(value) {
  return exactKeys(value, ["boot", "gpu_detection", "inference"])
    && validOutcome(value.boot)
    && validOutcome(value.gpu_detection)
    && validOutcome(value.inference);
}

function validPrivacy(value) {
  const falseFields = [
    "hostname", "username", "ip_or_mac", "serial_numbers", "credentials",
    "raw_logs", "prompts_or_model_output",
  ];
  return exactKeys(value, ["allowlisted_fields_only", ...falseFields])
    && value.allowlisted_fields_only === true
    && falseFields.every((key) => value[key] === false);
}

function validReport(value, nowSeconds) {
  return exactKeys(
    value,
    [
      "schema_version", "report_id", "generated_at_unix", "artifact",
      "platform", "hardware", "outcomes", "privacy",
    ],
    ["note", "error_excerpt"],
  )
    && value.schema_version === REPORT_SCHEMA
    && boundedString(value.report_id, 28, /^chr_[0-9a-f]{24}$/)
    && safeInteger(value.generated_at_unix, 1, nowSeconds + 86_400)
    && validArtifact(value.artifact)
    && validPlatform(value.platform)
    && validHardware(value.hardware)
    && validOutcomes(value.outcomes)
    && optionalString(value, "note", 1000, undefined, true)
    && optionalString(value, "error_excerpt", 2000, undefined, true)
    && validPrivacy(value.privacy);
}

function validCredit(value) {
  if (!exactKeys(value, ["publish", "permanence_acknowledged"], ["display_name"])) {
    return false;
  }
  if (typeof value.publish !== "boolean" || typeof value.permanence_acknowledged !== "boolean") {
    return false;
  }
  if (value.publish) {
    return value.permanence_acknowledged === true
      && boundedString(value.display_name, 80);
  }
  return value.permanence_acknowledged === false && !Object.hasOwn(value, "display_name");
}

export function validateSubmission(value, nowSeconds = Math.floor(Date.now() / 1000)) {
  return exactKeys(value, ["schema_version", "report", "credit_request"])
    && value.schema_version === SUBMISSION_SCHEMA
    && validReport(value.report, nowSeconds)
    && validCredit(value.credit_request);
}

async function rateKey(request, salt) {
  const address = request.headers.get("CF-Connecting-IP") || "unknown";
  const bytes = new TextEncoder().encode(`${salt}\0${address}`);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

async function applyRateLimit(context, nowSeconds) {
  const key = await rateKey(context.request, context.env.RATE_LIMIT_SALT);
  const windowStart = Math.floor(nowSeconds / 3600) * 3600;
  await context.env.REPORTS.prepare(
    "DELETE FROM report_rate_limits WHERE window_start < ?",
  ).bind(windowStart - 3600).run();
  const count = await context.env.REPORTS.prepare(
    `INSERT INTO report_rate_limits (rate_key, window_start, request_count)
     VALUES (?, ?, 1)
     ON CONFLICT(rate_key, window_start)
     DO UPDATE SET request_count = request_count + 1
     RETURNING request_count`,
  ).bind(key, windowStart).first("request_count");
  return typeof count === "number" && count <= RATE_LIMIT_PER_HOUR;
}

export async function onRequest(context) {
  if (context.request.method !== "POST") {
    return response(405, { error: "method_not_allowed" });
  }
  if (!context.env.REPORTS || !context.env.RATE_LIMIT_SALT) {
    return response(503, { error: "intake_not_configured" });
  }
  const contentType = context.request.headers.get("Content-Type") || "";
  if (!contentType.toLowerCase().startsWith("application/json")) {
    return response(415, { error: "content_type_must_be_json" });
  }
  const declaredLength = Number(context.request.headers.get("Content-Length") || 0);
  if (!Number.isFinite(declaredLength) || declaredLength > MAX_BODY_BYTES) {
    return response(413, { error: "submission_too_large" });
  }

  const body = await context.request.arrayBuffer();
  if (body.byteLength > MAX_BODY_BYTES) {
    return response(413, { error: "submission_too_large" });
  }
  const nowSeconds = Math.floor(Date.now() / 1000);
  try {
    if (!(await applyRateLimit(context, nowSeconds))) {
      return response(429, { error: "rate_limited" }, { "Retry-After": "3600" });
    }
  } catch (error) {
    console.error("hardware report rate limit failed", error);
    return response(500, { error: "intake_failed" });
  }
  let submission;
  try {
    submission = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(body));
  } catch {
    return response(400, { error: "invalid_json" });
  }
  if (!validateSubmission(submission, nowSeconds)) {
    return response(422, { error: "invalid_submission" });
  }

  try {
    await context.env.REPORTS.prepare(
      `INSERT INTO pending_hardware_reports
       (report_id, submitted_at_unix, report_json, credit_publish, credit_display_name, status)
       VALUES (?, ?, ?, ?, ?, 'pending')`,
    ).bind(
      submission.report.report_id,
      nowSeconds,
      JSON.stringify(submission.report),
      submission.credit_request.publish ? 1 : 0,
      submission.credit_request.display_name || null,
    ).run();
  } catch (error) {
    if (String(error).includes("UNIQUE constraint failed")) {
      return response(409, { error: "report_already_received" });
    }
    console.error("hardware report insert failed", error);
    return response(500, { error: "intake_failed" });
  }
  return response(202, {
    status: "pending_moderation",
    receipt: submission.report.report_id,
  });
}
