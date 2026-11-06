-- expect-error-nata: cannot compare
SELECT id FROM '{data}/sqlcheck/types.parquet' WHERE i = '1'
