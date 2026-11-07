SELECT m, count(*) AS n, sum(big) AS s, min(q) AS lo, max(q) AS hi FROM '{data}/sqlcheck/big.parquet' GROUP BY m HAVING count(*) > 2;
