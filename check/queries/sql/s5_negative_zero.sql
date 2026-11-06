SELECT id, d = CAST(0 AS DOUBLE) AS a, d < CAST(0 AS DOUBLE) AS b, r = r AS c FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1, 10)
