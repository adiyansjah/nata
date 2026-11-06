-- expect-error-nata: not a valid INTEGER
SELECT CAST('1_000' AS INTEGER) AS a FROM '{data}/sqlcheck/types.parquet'
