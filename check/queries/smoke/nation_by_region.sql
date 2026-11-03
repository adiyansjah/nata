SELECT n_regionkey, count(*) AS n
FROM '{data}/nation.parquet'
GROUP BY n_regionkey
ORDER BY n_regionkey
