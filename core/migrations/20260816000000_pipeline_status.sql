ALTER TABLE worksheets ADD COLUMN pipeline_status TEXT NOT NULL DEFAULT 'idle';
ALTER TABLE worksheets ADD COLUMN pipeline_error TEXT;