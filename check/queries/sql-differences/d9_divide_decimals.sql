-- tolerance: 1e-7
SELECT id, dec52 / 3 AS a FROM '{data}/sqlcheck/types.parquet' WHERE id IN (2,3,7,14)
