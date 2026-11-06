SELECT id FROM '{data}/sqlcheck/types.parquet' WHERE i IN (1, 2, 3, 100) AND s NOT IN ('a')
