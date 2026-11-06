-- expect-error-nata: scale
SELECT CAST(1 AS DECIMAL(20,20)) * CAST(1 AS DECIMAL(20,20)) AS a FROM '{data}/sqlcheck/types.parquet'
