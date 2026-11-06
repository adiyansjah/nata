-- expect-error
SELECT i % -1 AS a FROM '{data}/sqlcheck/types.parquet'
