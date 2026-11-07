-- expect-error-nata: overflow
SELECT sum(i + 1) AS s FROM '{data}/sqlcheck/types.parquet';
