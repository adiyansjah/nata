-- expect-error
SELECT CAST('2147483648' AS INTEGER) AS a FROM '{data}/sqlcheck/types.parquet'
