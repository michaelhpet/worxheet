-- Segmentation replaces HDBSCAN clustering as the unit of generation.
-- Derived rows were built under the old scheme and cannot be reused, so they
-- are cleared; completed worksheets are re-kicked through the new pipeline at
-- startup by `resume_stale`.
DROP TABLE IF EXISTS clusters;

DELETE FROM artifacts;
DELETE FROM chunks;

ALTER TABLE chunks ADD COLUMN heading TEXT;
ALTER TABLE chunks DROP COLUMN cluster_index;

UPDATE worksheets SET pipeline_status = 'running', pipeline_error = NULL
WHERE pipeline_status = 'done'
  AND EXISTS (SELECT 1 FROM files WHERE files.worksheet_id = worksheets.id);
