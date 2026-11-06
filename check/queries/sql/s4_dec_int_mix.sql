SELECT id, dec52 + i AS a, dec52 * ti AS b, 1 - dec52 AS c, dec183 + 1 AS d FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1,2,3,7,8)
