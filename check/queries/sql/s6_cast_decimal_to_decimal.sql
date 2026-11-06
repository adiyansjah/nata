SELECT id, CAST(dec52 AS DECIMAL(10,5)) AS a, CAST(dec3810 AS DECIMAL(38,2)) AS b FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1,2,3,6,7)
