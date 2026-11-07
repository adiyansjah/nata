SELECT f, count(*) AS n, count(i) AS c, sum(i) AS s FROM '{data}/sqlcheck/empty.parquet' GROUP BY f;
