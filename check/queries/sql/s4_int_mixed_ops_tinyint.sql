SELECT id, ti - 3 AS a, ti * 2 AS b, ti % 3 AS c, ti // 2 AS d FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1, 2, 3, 6, 7, 8)
