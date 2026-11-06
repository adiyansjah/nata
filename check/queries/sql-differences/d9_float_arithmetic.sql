-- expect-error-nata: arithmetic on DOUBLE
SELECT d + d AS a FROM '{data}/sqlcheck/types.parquet'
