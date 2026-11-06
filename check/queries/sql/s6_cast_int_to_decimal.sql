SELECT id, CAST(ti AS DECIMAL(10,2)) AS a, CAST(i AS DECIMAL(12,0)) AS b FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1,2,3,6,7)
