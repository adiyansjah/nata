SELECT f, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY f HAVING (f OR f IS NULL) AND NOT (count(*) < 2);
