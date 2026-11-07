SELECT f, count(*) AS n, count(i) AS c, sum(i) AS s FROM '{data}/sqlcheck/all_null.parquet' GROUP BY f;
