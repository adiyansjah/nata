SELECT s, count(*) AS n, sum(big) AS total
FROM '{data}/sqlcheck/big.parquet'
GROUP BY s HAVING count(*) > 100;
