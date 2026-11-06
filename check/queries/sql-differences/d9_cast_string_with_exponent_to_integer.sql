-- expect-error-nata: not a valid BIGINT
SELECT CAST('1e2' AS BIGINT) AS a FROM '{data}/sqlcheck/types.parquet'
