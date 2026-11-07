SELECT f FROM '{data}/sqlcheck/types.parquet' GROUP BY f HAVING count(i) > 3;
