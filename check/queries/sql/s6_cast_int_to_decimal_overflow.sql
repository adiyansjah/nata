-- expect-error
SELECT CAST(i AS DECIMAL(5,2)) AS a FROM '{data}/sqlcheck/types.parquet' WHERE id = 4
