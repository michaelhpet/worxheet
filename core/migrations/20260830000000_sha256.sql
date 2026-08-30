-- Content hash for files, used to reuse already-ingested chunks for the same
-- source file instead of re-parsing/segmenting it per worksheet.
ALTER TABLE files ADD COLUMN sha256 TEXT;
CREATE INDEX idx_files_sha256 ON files(sha256);
