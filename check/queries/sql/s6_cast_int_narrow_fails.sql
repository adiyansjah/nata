-- expect-error
SELECT CAST(b AS INTEGER) AS a FROM '{data}/sqlcheck/types.parquet'
