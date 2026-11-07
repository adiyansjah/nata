-- expect-error-nata: integer overflow
SELECT - -2147483648 AS x FROM '{data}/sqlcheck/one_row.parquet' LIMIT 1
