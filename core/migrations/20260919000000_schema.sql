-- Single source of truth for the Worxheet database schema.
-- Squashed from the 2026-04..2026-08 incremental migrations. Dead weight
-- removed in the squash: `clusters` table + `chunks.cluster_index`
-- (HDBSCAN replaced by segmentation), `chunks.embedding` (embed stage
-- removed, never read), `files.status` (never read/written).
-- NOTE: this replaces migration history. Existing local `database.sqlite`
-- files must be deleted once; they cannot migrate forward from the old chain.
CREATE TABLE worksheets (
    id TEXT NOT NULL PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    pipeline_status TEXT NOT NULL DEFAULT 'idle',
    pipeline_error TEXT
);

CREATE TABLE files (
    id TEXT NOT NULL PRIMARY KEY,
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    extension TEXT NOT NULL,
    size INTEGER NOT NULL,
    identity_key TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_files_worksheet ON files(worksheet_id);
CREATE INDEX idx_files_identity ON files(identity_key);

CREATE TABLE chunks (
    id TEXT NOT NULL PRIMARY KEY,
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    heading TEXT,
    text TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_chunks_worksheet ON chunks(worksheet_id);
CREATE INDEX idx_chunks_worksheet_position ON chunks(worksheet_id, position);

CREATE TABLE artifacts (
    id TEXT NOT NULL PRIMARY KEY,
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id) ON DELETE CASCADE,
    artifact_type TEXT NOT NULL,
    source TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_artifacts_worksheet_type ON artifacts(worksheet_id, artifact_type);
