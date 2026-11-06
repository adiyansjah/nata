-- expect-error
SELECT CAST('2023-02-29' AS DATE) AS a FROM '{data}/sqlcheck/types.parquet'
