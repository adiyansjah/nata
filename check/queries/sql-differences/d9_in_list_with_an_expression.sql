-- expect-error-nata: not a literal in an IN list
SELECT id FROM '{data}/sqlcheck/types.parquet' WHERE i IN (id, 2)
