SELECT id, CAST(d AS DECIMAL(10,2)) AS a FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1,2,3,6,12,14,15)
