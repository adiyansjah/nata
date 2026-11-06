SELECT id, i - 3 AS a, i * 2 AS b, i % 3 AS c, i // 2 AS d FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1, 2, 3, 6, 7, 8)
