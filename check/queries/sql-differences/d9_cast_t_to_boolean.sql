-- expect-error-nata: not a valid BOOLEAN
SELECT CAST('t' AS BOOLEAN) AS a FROM '{data}/sqlcheck/types.parquet'
