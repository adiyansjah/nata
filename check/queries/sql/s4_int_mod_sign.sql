SELECT id, i % 3 AS a, i % -3 AS b, -7 % 3 AS c FROM '{data}/sqlcheck/types.parquet' WHERE id IN (2,3,5,7,8)
