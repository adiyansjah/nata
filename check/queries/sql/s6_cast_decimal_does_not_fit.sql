-- expect-error
SELECT CAST(dec183 AS DECIMAL(5,2)) AS a FROM '{data}/sqlcheck/types.parquet' WHERE id = 4
