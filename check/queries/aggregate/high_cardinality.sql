SELECT i, count(*) AS n, sum(big) AS s FROM '{data}/sqlcheck/big.parquet' GROUP BY i HAVING count(*) > 0;
