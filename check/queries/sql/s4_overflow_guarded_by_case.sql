SELECT CASE WHEN i < 2147483647 THEN i + 1 ELSE 0 END AS a FROM '{data}/sqlcheck/types.parquet' WHERE i IS NOT NULL
