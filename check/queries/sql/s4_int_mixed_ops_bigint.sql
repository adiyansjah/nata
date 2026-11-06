SELECT id, b - 3 AS a, b * 2 AS b, b % 3 AS c, b // 2 AS d FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1, 2, 3, 6, 7, 8)
