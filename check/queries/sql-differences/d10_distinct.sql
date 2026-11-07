-- expect-error-nata: DISTINCT aggregates
SELECT count(DISTINCT i) AS v FROM '{data}/sqlcheck/types.parquet';
