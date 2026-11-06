-- expect-error-nata: not a valid INTEGER
SELECT CAST('1.5' AS INTEGER) AS a FROM '{data}/sqlcheck/types.parquet'
