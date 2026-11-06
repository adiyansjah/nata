-- tolerance: 0
SELECT id, dec52 % 0.7 AS a FROM '{data}/sqlcheck/types.parquet' WHERE id IN (2,3,7,8,12)
