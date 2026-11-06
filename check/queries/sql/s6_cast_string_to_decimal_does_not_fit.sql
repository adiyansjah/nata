-- expect-error
SELECT CAST('1000' AS DECIMAL(5,2)) AS a FROM '{data}/sqlcheck/types.parquet'
