SELECT id, si - 3 AS a, si * 2 AS b, si % 3 AS c, si // 2 AS d FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1, 2, 3, 6, 7, 8)
