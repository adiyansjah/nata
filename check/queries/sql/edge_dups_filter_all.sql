SELECT id, i + 1 AS a FROM '{data}/sqlcheck/dups.parquet' WHERE i IS NULL OR i IS NOT NULL
