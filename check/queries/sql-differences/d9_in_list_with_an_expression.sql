-- expect-error-nata: not supported yet
SELECT id FROM '{data}/sqlcheck/types.parquet' WHERE i IN (id, 2)
