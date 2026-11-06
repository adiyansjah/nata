-- expect-error
SELECT CAST(s AS INTEGER) AS a FROM '{data}/sqlcheck/types.parquet'
