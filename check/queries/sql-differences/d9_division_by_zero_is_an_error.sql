-- expect-error-nata: division by zero
SELECT i / 0 AS a FROM '{data}/sqlcheck/types.parquet'
