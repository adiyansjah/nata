SELECT id, CAST(b AS INTEGER) AS a FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1, 2, 3, 6)
