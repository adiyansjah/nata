-- expect-error
SELECT CAST(d AS DECIMAL(10,2)) AS a FROM '{data}/sqlcheck/types.parquet' WHERE id = 7
