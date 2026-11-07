SELECT f, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY f HAVING count(*) > 4;
