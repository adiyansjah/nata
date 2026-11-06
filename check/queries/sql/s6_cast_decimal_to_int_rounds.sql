SELECT id, CAST(dec52 AS INTEGER) AS a, CAST(dec183 AS BIGINT) AS b FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1,2,3,6,7,9,10,12)
