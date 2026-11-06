-- expect-error-nata: comparing DOUBLE with INTEGER
SELECT id FROM '{data}/sqlcheck/types.parquet' WHERE d > 1
