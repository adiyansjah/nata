-- expect-error-nata: GROUP BY ordinals
SELECT i % 2 AS k, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY 1;
