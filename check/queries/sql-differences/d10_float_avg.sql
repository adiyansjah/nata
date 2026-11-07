-- expect-error-nata: avg(FLOAT)
SELECT avg(r) AS v FROM '{data}/sqlcheck/types.parquet';
