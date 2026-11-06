SELECT id, CAST(r AS DOUBLE) AS a FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1,2,3,6,7,8,9,10,12)
