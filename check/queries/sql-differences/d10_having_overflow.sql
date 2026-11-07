-- expect-error-nata: overflow
SELECT sum(CAST(99999999999999999999999999999999999999 AS DECIMAL(38,0))) AS s FROM '{data}/sqlcheck/types.parquet' HAVING false;
