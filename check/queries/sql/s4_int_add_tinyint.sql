SELECT id, ti + 1 AS v FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1, 2, 3, 6, 7)
