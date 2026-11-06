SELECT id, 1 + 2 * 3 - 4 AS a, (1 + 2) * 3 AS b, -2 * -3 AS c, 10 - 2 - 3 AS d FROM '{data}/sqlcheck/types.parquet' LIMIT 1
