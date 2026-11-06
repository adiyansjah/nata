SELECT id, CASE WHEN i = 0 THEN 0 ELSE 100 // i END AS a FROM '{data}/sqlcheck/types.parquet' WHERE i IS NOT NULL
