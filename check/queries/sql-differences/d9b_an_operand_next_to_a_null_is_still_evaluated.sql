-- expect-error-nata: integer overflow
SELECT i * 2147483647 + CAST(NULL AS INTEGER) AS x FROM '{data}/sqlcheck/types.parquet' WHERE id = 4
