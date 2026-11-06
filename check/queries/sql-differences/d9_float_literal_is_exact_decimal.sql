-- tolerance: 0
SELECT id, 1.5e0 AS a, 1e2 AS b, 2.5E-1 AS c FROM '{data}/sqlcheck/types.parquet' LIMIT 2
