-- expect-error-nata: integer overflow
SELECT -(-9223372036854775808) AS x FROM '{data}/sqlcheck/one_row.parquet' LIMIT 1
