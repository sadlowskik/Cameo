CREATE TABLE IF NOT EXISTS pending_hardware_reports (
  report_id TEXT PRIMARY KEY NOT NULL,
  submitted_at_unix INTEGER NOT NULL,
  report_json TEXT NOT NULL,
  credit_publish INTEGER NOT NULL CHECK (credit_publish IN (0, 1)),
  credit_display_name TEXT,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'rejected')),
  reviewed_at_unix INTEGER,
  reviewer_note TEXT,
  CHECK (
    (credit_publish = 1 AND credit_display_name IS NOT NULL)
    OR (credit_publish = 0 AND credit_display_name IS NULL)
  )
);

CREATE INDEX IF NOT EXISTS pending_hardware_reports_status_submitted
  ON pending_hardware_reports (status, submitted_at_unix);

CREATE TABLE IF NOT EXISTS report_rate_limits (
  rate_key TEXT NOT NULL,
  window_start INTEGER NOT NULL,
  request_count INTEGER NOT NULL CHECK (request_count > 0),
  PRIMARY KEY (rate_key, window_start)
);

CREATE INDEX IF NOT EXISTS report_rate_limits_window
  ON report_rate_limits (window_start);
