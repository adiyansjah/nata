SELECT id, i + 1 AS a FROM '{data}/sqlcheck/empty.parquet' WHERE i IS NULL OR i IS NOT NULL
