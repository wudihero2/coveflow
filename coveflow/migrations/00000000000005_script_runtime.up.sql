-- Add runtime column to script table.
-- Stores the container image tag used for execution, e.g. 'python:3.12', 'python:3.11'.
-- NULL means "use the platform default for this language".
ALTER TABLE script ADD COLUMN runtime VARCHAR(255);
