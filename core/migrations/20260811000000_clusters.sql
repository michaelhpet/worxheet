ALTER TABLE chunks ADD COLUMN cluster_index INTEGER;

CREATE TABLE clusters (
    worksheet_id TEXT NOT NULL REFERENCES worksheets(id) ON DELETE CASCADE,
    cluster_index INTEGER NOT NULL,
    centroid BLOB NOT NULL,
    size INTEGER NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (worksheet_id, cluster_index)
);

CREATE INDEX idx_clusters_worksheet ON clusters(worksheet_id);