-- expect-error-nata: not a valid BOOLEAN
SELECT CAST('yes' AS BOOLEAN) AS a FROM '{data}/sqlcheck/types.parquet'
