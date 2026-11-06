-- expect-error-nata: cannot compare
SELECT id FROM '{data}/sqlcheck/types.parquet' WHERE dt = '1994-01-01'
