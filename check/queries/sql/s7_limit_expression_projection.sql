SELECT id, i + 1 AS a FROM '{data}/sqlcheck/types.parquet' WHERE i < 100 LIMIT 5
