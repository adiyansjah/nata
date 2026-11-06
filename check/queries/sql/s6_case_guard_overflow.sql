SELECT id, CASE WHEN i = 2147483647 THEN 0 ELSE i + 1 END AS a FROM '{data}/sqlcheck/types.parquet' WHERE i IS NOT NULL
