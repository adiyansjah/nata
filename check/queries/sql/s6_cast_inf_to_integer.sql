-- expect-error
SELECT CAST(d AS INTEGER) AS a FROM '{data}/sqlcheck/types.parquet' WHERE id = 8
