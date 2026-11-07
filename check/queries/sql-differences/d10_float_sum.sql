-- expect-error-nata: sum(DOUBLE)
SELECT sum(d) AS v FROM '{data}/sqlcheck/types.parquet';
