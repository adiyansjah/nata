SELECT f, count(*) AS n, count(i) AS c, sum(i) AS s FROM '{data}/sqlcheck/one_row.parquet' GROUP BY f;
