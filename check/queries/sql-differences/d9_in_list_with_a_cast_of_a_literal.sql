-- expect-error-nata: not supported yet
SELECT id, d IN (CAST(1.5 AS DOUBLE)) AS a FROM '{data}/sqlcheck/types.parquet'
