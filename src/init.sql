CREATE TABLE IF NOT EXISTS `paths` (
    path BLOB NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_paths_path ON paths (path);
