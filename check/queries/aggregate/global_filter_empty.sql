SELECT count(*) AS n, sum(i) AS s FROM '{data}/sqlcheck/types.parquet' WHERE false;
