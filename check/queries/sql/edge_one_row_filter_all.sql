SELECT id, i + 1 AS a FROM '{data}/sqlcheck/one_row.parquet' WHERE i IS NULL OR i IS NOT NULL
