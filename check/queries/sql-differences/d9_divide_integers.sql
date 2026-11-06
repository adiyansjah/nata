-- tolerance: 1e-6
SELECT id, i / 2 AS a, 7 / 2 AS b FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1,2,3,7)
