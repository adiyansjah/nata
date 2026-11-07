SELECT f, count(*) AS n, count(i) AS c, sum(i) AS s FROM '{data}/sqlcheck/dups.parquet' GROUP BY f;
