-- expect-error-nata: aggregate FILTER
SELECT count(*) FILTER (WHERE i > 0) AS v FROM '{data}/sqlcheck/types.parquet';
