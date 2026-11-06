-- tolerance: 0.0100000001
SELECT CAST(CAST(2.675 AS DOUBLE) AS DECIMAL(5,2)) AS a FROM '{data}/sqlcheck/types.parquet' LIMIT 1
